//! Author pictures. A GitHub no-reply address (`123+name@users.noreply.github.com`) shows that account's GitHub
//! picture; any other address is looked up on Gravatar by the SHA-256 of the trimmed, lower-case address (only the hash
//! is sent). An address without a picture answers 404 and the app shows initials.

use sha2::{Digest, Sha256};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
/// Larger answers are not pictures GitCraft asked for.
const MAX_BYTES: usize = 512 * 1024;

/// The picture's address at `px` pixels, or `None` for something that isn't an address.
pub fn url(email: &str, px: u32) -> Option<String> {
    let email = email.trim().to_lowercase();
    if !email.contains('@') {
        return None;
    }
    let hash: String = Sha256::digest(email.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("https://gravatar.com/avatar/{hash}?s={px}&d=404"))
}

/// The GitHub picture of a GitHub no-reply address: by account id (`123+name@…`), else by login (`name@…`).
pub fn github_url(email: &str, px: u32) -> Option<String> {
    let email = email.trim().to_lowercase();
    let local = email.strip_suffix("@users.noreply.github.com")?;
    let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    match local.split_once('+') {
        Some((id, _)) if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) => Some(format!("https://avatars.githubusercontent.com/u/{id}?s={px}")),
        None if valid(local) => Some(format!("https://github.com/{local}.png?size={px}")),
        _ => None,
    }
}

/// The picture's bytes, `Ok(None)` when the address has none.
pub fn fetch(email: &str, px: u32) -> Result<Option<Vec<u8>>, String> {
    let Some(url) = github_url(email, px).or_else(|| url(email, px)) else { return Ok(None) };
    let _ = rustls::crypto::ring::default_provider().install_default();
    crate::runtime().block_on(async {
        let policy = reqwest::redirect::Policy::custom(|a| if a.previous().len() > 4 || a.url().scheme() != "https" { a.stop() } else { a.follow() });
        let client = reqwest::Client::builder().redirect(policy).timeout(TIMEOUT).user_agent(concat!("GitCraft/", env!("CARGO_PKG_VERSION"))).build().map_err(|e| e.to_string())?;
        let mut response = client.get(&url).send().await.map_err(crate::hosting::transport_error)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!("Gravatar answered {}.", response.status()));
        }
        // Read piece by piece and stop at the limit, whether or not the size was announced.
        let too_large = || Err("The picture is too large.".to_owned());
        if response.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
            return too_large();
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(crate::hosting::transport_error)? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return too_large();
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(bytes))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_hashed_trimmed_and_lower_case() {
        // SHA-256 of "test@example.com".
        let want = "https://gravatar.com/avatar/973dfe463ec85785f5f95af5ba3906eedb2d931c24e69824a89ea65dba4e813b?s=64&d=404";
        assert_eq!(url("  Test@Example.com ", 64).as_deref(), Some(want));
        assert_eq!(url("not an address", 64), None);
        assert_eq!(url("", 64), None);
    }

    #[test]
    fn github_no_reply_addresses_use_the_github_picture() {
        assert_eq!(github_url("12345+octo@users.noreply.github.com", 64).as_deref(), Some("https://avatars.githubusercontent.com/u/12345?s=64"));
        assert_eq!(github_url("octo-cat@users.noreply.github.com", 64).as_deref(), Some("https://github.com/octo-cat.png?size=64"));
        assert_eq!(github_url("someone@example.com", 64), None);
        assert_eq!(github_url("../x@users.noreply.github.com", 64), None);
    }
}
