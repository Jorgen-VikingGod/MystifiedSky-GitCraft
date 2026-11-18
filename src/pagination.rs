//! Follow collection continuations without sending credentials to another origin.
use crate::hosting::HttpPage;
use serde_json::Value;

fn origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return String::new() };
    format!("{}://{}", scheme.to_ascii_lowercase(), rest.split('/').next().unwrap_or_default().to_ascii_lowercase())
}

pub(crate) fn parameter(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?.1.split('&').find_map(|part| part.split_once('=').filter(|(name, _)| *name == key).map(|(_, value)| value.to_owned()))
}

pub(crate) fn with_parameter(url: &str, key: &str, value: &str) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut parts: Vec<_> = query.split('&').filter(|part| !part.is_empty() && part.split('=').next() != Some(key)).map(str::to_owned).collect();
    parts.push(format!("{key}={value}"));
    format!("{base}?{}", parts.join("&"))
}

fn array_key(value: &Value) -> Option<&str> {
    if value.is_array() { return Some(""); }
    ["values", "value", "items", "issues"].into_iter().find(|key| value[*key].is_array())
}

fn next(url: &str, page: &HttpPage, count: usize) -> Option<String> {
    if let Some(link) = &page.link {
        if let Some(part) = link.split(',').find(|part| part.contains("rel=\"next\"")) {
            if let Some((_, rest)) = part.split_once('<') { return rest.split_once('>').map(|(url, _)| url.to_owned()); }
        }
    }
    if let Some(value) = page.value["nextPageToken"].as_str().filter(|v| !v.is_empty()) { return Some(with_parameter(url, "nextPageToken", &crate::hosting_api::q(value))); }
    if let Some(value) = page.value["next"].as_str().filter(|s| !s.is_empty()) { return Some(value.into()); }
    if let Some(value) = page.next_page.as_deref().filter(|s| !s.is_empty()) { return Some(with_parameter(url, "page", value)); }
    if let Some(value) = page.continuation.as_deref().filter(|s| !s.is_empty()) { return Some(with_parameter(url, "continuationToken", &crate::hosting_api::q(value))); }
    if page.value["isLastPage"] == false {
        if let Some(start) = page.value["nextPageStart"].as_u64() { return Some(with_parameter(url, "start", &start.to_string())); }
    }
    if let (Some(start), Some(total), Some(limit)) = (page.value["startAt"].as_u64(), page.value["total"].as_u64(), page.value["maxResults"].as_u64()) {
        if count > 0 && start + limit < total { return Some(with_parameter(url, "startAt", &(start + limit).to_string())); }
    }
    if let Some(size) = parameter(url, "per_page").and_then(|s| s.parse::<usize>().ok()) {
        if size > 0 && count >= size {
            let page = parameter(url, "page").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
            return Some(with_parameter(url, "page", &(page + 1).to_string()));
        }
    }
    if let Some(size) = parameter(url, "$top").and_then(|s| s.parse::<usize>().ok()) {
        if size > 0 && count >= size { let skip = parameter(url, "$skip").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0); return Some(with_parameter(url, "$skip", &(skip + size).to_string())); }
    }
    None
}

const COLLECTION_BYTES: usize = 64 * 1024 * 1024;
const CURSOR_BYTES: usize = 16 * 1024 * 1024;
fn json_bytes(value: &Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> { self.0 += bytes.len(); Ok(bytes.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut count = Count(0); let _ = serde_json::to_writer(&mut count, value); count.0
}

struct PageMode { input: Option<String>, output: Option<String>, used: bool }
thread_local! { static PAGE_MODE: std::cell::RefCell<Option<PageMode>> = const { std::cell::RefCell::new(None) }; }
/// Reuse provider decoders for one-page collection reads. Complete lookup callers
/// continue to use collect outside this scope.
pub fn one_page<T>(cursor: Option<String>, read: impl FnOnce() -> Result<T, String>) -> Result<Page<T>, String> {
    struct Restore(Option<PageMode>);
    impl Drop for Restore { fn drop(&mut self) { PAGE_MODE.with(|mode| mode.replace(self.0.take())); } }
    let _restore = Restore(PAGE_MODE.with(|mode| mode.replace(Some(PageMode { input: cursor, output: None, used: false }))));
    let items = read()?;
    let cursor = PAGE_MODE.with(|mode| mode.borrow_mut().as_mut().and_then(|m| m.output.take()));
    Ok(Page { items, cursor })
}

#[derive(Clone)]
struct Source { cursor: Option<String>, complete: bool, empty: Value }
#[derive(Clone)]
struct Composite { previous: Option<String>, binding: String, sources: std::collections::HashMap<String, Source>, deadline: std::time::Instant }
thread_local! { static COMPOSITE: std::cell::RefCell<Option<Composite>> = const { std::cell::RefCell::new(None) }; }
fn composites() -> &'static std::sync::Mutex<std::collections::HashMap<String, Composite>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Composite>>> = std::sync::OnceLock::new(); CACHE.get_or_init(Default::default)
}
fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut hash = std::collections::hash_map::RandomState::new().build_hasher(); hash.write_u64(SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)); format!("{:016x}", hash.finish())
}
/// A provider may combine several collections. Continue each unfinished source
/// once per request; completed sources yield an empty collection of the same shape.
pub fn composite_page<T>(binding: &str, cursor: Option<&str>, read: impl FnOnce() -> Result<T, String>) -> Result<Page<T>, String> {
    let state = if let Some(cursor) = cursor {
        composites().lock().unwrap().get(cursor).filter(|s| s.binding == binding && s.deadline > std::time::Instant::now()).cloned().ok_or("This result cursor expired; refresh the collection.")?
    } else { Composite { previous: None, binding: binding.into(), sources: Default::default(), deadline: std::time::Instant::now() + std::time::Duration::from_secs(120) } };
    struct Restore(Option<Composite>);
    impl Drop for Restore { fn drop(&mut self) { COMPOSITE.with(|c| c.replace(self.0.take())); } }
    let _restore = Restore(COMPOSITE.with(|c| c.replace(Some(state))));
    let items = read()?; crate::read_jobs::check()?;
    let mut state = COMPOSITE.with(|c| c.borrow().as_ref().unwrap().clone());
    if std::time::Instant::now() >= state.deadline { return Err("The collection exceeded its two-minute budget; refresh the collection.".into()); }
    let next = if state.sources.values().any(|s| !s.complete) {
        let mut cache = composites().lock().unwrap(); cache.retain(|_, s| s.deadline > std::time::Instant::now());
        let obsolete = cursor.and_then(|key| cache.get(key)).and_then(|s| s.previous.clone());
        let cost = |s: &Composite| s.binding.len() + s.sources.iter().map(|(url, source)| url.len() + json_bytes(&source.empty) + 128).sum::<usize>() + 256;
        let retained: usize = cache.iter().filter(|(key, _)| obsolete.as_deref() != Some(key.as_str())).map(|(_, s)| cost(s)).sum();
        if state.sources.len() > 64 || retained + cost(&state) > CURSOR_BYTES || (cache.len() >= 128 && obsolete.is_none()) { return Err("Too many active result cursors; refresh or narrow the collection.".into()); }
        if let Some(obsolete) = obsolete { cache.remove(&obsolete); }
        state.previous = cursor.map(str::to_owned);
        let token = token(); cache.insert(token.clone(), state); Some(token)
    } else { None };
    // Keep the previous token for retries after interrupted IPC delivery. Its
    // child cursors retain one predecessor and all states expire automatically.
    Ok(Page { items, cursor: next })
}
fn composite_collect(start: &str, fetch: impl FnMut(&str) -> Result<HttpPage, String>) -> Result<Value, String> {
    let previous = COMPOSITE.with(|c| c.borrow().as_ref().unwrap().sources.get(start).cloned());
    if let Some(source) = &previous { if source.complete { return Ok(source.empty.clone()); } }
    let result = page(start, previous.as_ref().and_then(|s| s.cursor.as_deref()), fetch);
    match result {
        Ok(page) => {
            if let Some(key) = array_key(&page.items) {
                let empty = if key.is_empty() { serde_json::json!([]) } else { let mut value = serde_json::Map::new(); value.insert(key.into(), serde_json::json!([])); Value::Object(value) };
                COMPOSITE.with(|c| c.borrow_mut().as_mut().unwrap().sources.insert(start.into(), Source { complete: page.cursor.is_none(), cursor: page.cursor, empty }));
            }
            Ok(page.items)
        }
        Err(error) => {
            COMPOSITE.with(|c| { c.borrow_mut().as_mut().unwrap().sources.entry(start.into()).or_insert(Source { cursor: None, complete: false, empty: Value::Null }); });
            Err(error)
        }
    }
}

pub fn collect(start: &str, mut fetch: impl FnMut(&str) -> Result<HttpPage, String>) -> Result<Value, String> {
    if COMPOSITE.with(|c| c.borrow().is_some()) { return composite_collect(start, fetch); }
    let paged = PAGE_MODE.with(|mode| {
        let mut mode = mode.borrow_mut();
        mode.as_mut().map(|m| { let used = m.used; m.used = true; (used, m.input.take()) })
    });
    if let Some((used, cursor)) = paged {
        if used { return Err("This provider needs a composite result cursor.".into()); }
        let result = page(start, cursor.as_deref(), fetch)?;
        PAGE_MODE.with(|mode| mode.borrow_mut().as_mut().unwrap().output = result.cursor);
        return Ok(result.items);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut current = start.to_owned();
    let mut visited = std::collections::HashSet::new();
    let mut combined: Option<Value> = None;
    let mut bytes = 0;
    loop {
        crate::read_jobs::check()?;
        if std::time::Instant::now() >= deadline { return Err("The collection exceeded its two-minute budget; narrow the query.".into()); }
        if visited.len() >= 1000 { return Err("The service exceeded 1000 result pages; narrow the query.".into()); }
        if !visited.insert(current.clone()) { return Err("The service repeated a pagination URL; results may be incomplete.".into()); }
        let mut page = fetch(&current)?;
        bytes += json_bytes(&page.value);
        if bytes > COLLECTION_BYTES { return Err("The collection exceeded its 64 MB budget; narrow the query.".into()); }
        let Some(key) = array_key(&page.value).map(str::to_owned) else { return Ok(page.value) };
        let rows = if key.is_empty() { page.value.as_array().unwrap() } else { page.value[&key].as_array().unwrap() };
        let following = next(&current, &page, rows.len());
        if let Some(total) = &mut combined {
            let destination = if key.is_empty() { total.as_array_mut() } else { total[&key].as_array_mut() }.ok_or("The service changed its pagination format.")?;
            let rows = if key.is_empty() { page.value.as_array_mut().unwrap() } else { page.value[&key].as_array_mut().unwrap() };
            destination.append(rows);
        } else { combined = Some(page.value); }
        let Some(following) = following else { return Ok(combined.unwrap()); };
        let following = if following.starts_with('/') { format!("{}{following}", origin(start)) } else { following };
        if following.len() > 8192 || origin(&following) != origin(start) { return Err("The service returned a pagination URL on a different host; credentials were not forwarded.".into()); }
        current = following;
    }
}


#[derive(serde::Serialize)]
pub struct Page<T> { pub items: T, pub cursor: Option<String> }
struct Continuation { start: String, current: String, visited: std::collections::HashSet<String>, deadline: std::time::Instant, bytes: usize, previous: Option<String> }
fn cursors() -> &'static std::sync::Mutex<std::collections::HashMap<String, Continuation>> {
    static CURSORS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Continuation>>> = std::sync::OnceLock::new();
    CURSORS.get_or_init(Default::default)
}
/// Cursors are server-owned, bound to the original collection and expire. A retry
/// uses the same token; successful continuation never consumes a partial result.
pub fn page(start: &str, cursor: Option<&str>, mut fetch: impl FnMut(&str) -> Result<HttpPage, String>) -> Result<Page<Value>, String> {
    let (current, mut visited, deadline, mut bytes) = if let Some(token) = cursor {
        let map = cursors().lock().unwrap();
        let state = map.get(token).filter(|s| s.start == start && s.deadline > std::time::Instant::now()).ok_or("This result cursor expired; refresh the collection.")?;
        (state.current.clone(), state.visited.clone(), state.deadline, state.bytes)
    } else { (start.to_string(), Default::default(), std::time::Instant::now() + std::time::Duration::from_secs(120), 0) };
    crate::read_jobs::check()?;
    if !visited.insert(current.clone()) || visited.len() > 1000 { return Err("The service repeated or exceeded its result pages.".into()); }
    let response = fetch(&current)?;
    bytes += json_bytes(&response.value);
    if bytes > COLLECTION_BYTES { return Err("The collection exceeded its 64 MB budget; narrow the query.".into()); }
    if std::time::Instant::now() >= deadline { return Err("The collection exceeded its two-minute budget; refresh or narrow the query.".into()); }
    let count = array_key(&response.value).and_then(|key| if key.is_empty() { response.value.as_array() } else { response.value[key].as_array() }).map(Vec::len).unwrap_or(0);
    let following = next(&current, &response, count).map(|url| if url.starts_with('/') { format!("{}{url}", origin(start)) } else { url });
    let token = if let Some(following) = following {
        if following.len() > 8192 || origin(&following) != origin(start) { return Err("The service returned a pagination URL on a different host; credentials were not forwarded.".into()); }
        if visited.contains(&following) { return Err("The service repeated a pagination URL.".into()); }
        use std::hash::{BuildHasher, Hasher};
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let token = format!("{:016x}", hasher.finish());
        let mut map = cursors().lock().unwrap();
        map.retain(|_, state| state.deadline > std::time::Instant::now());
        let obsolete = cursor.and_then(|key| map.get(key)).and_then(|state| state.previous.clone());
        let retained: usize = map.iter().filter(|(key, _)| obsolete.as_deref() != Some(key.as_str())).map(|(key, state)| key.len() + state.start.len() + state.current.len() + state.visited.iter().map(|url| url.len() + 64).sum::<usize>() + 256).sum();
        let cost = start.len() + following.len() + visited.iter().map(|url| url.len() + 64).sum::<usize>() + 256;
        if retained + cost > CURSOR_BYTES { return Err("Result cursors exceeded their memory budget; refresh or narrow the query.".into()); }
        if let Some(obsolete) = obsolete { map.remove(&obsolete); }
        if map.len() >= 512 { return Err("Too many active result cursors; refresh a collection after older cursors expire.".into()); }
        map.insert(token.clone(), Continuation { start: start.into(), current: following, visited, deadline, bytes, previous: cursor.map(str::to_owned) });
        Some(token)
    } else { None };
    Ok(Page { items: response.value, cursor: token })
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn page(value:Value)->HttpPage { HttpPage {value,link:None,next_page:None,continuation:None} }
    #[test]
    fn composite_sources_continue_independently_retry_and_reject_rebinding() {
        let read = |fail: bool| -> Result<Vec<Value>, String> {
            let mut result = Vec::new();
            let a = collect("https://example.invalid/a", |url| {
                let mut response = page(if url.contains("page=2") { json!([2]) } else { json!([1]) });
                if !url.contains("page=2") { response.link = Some("<https://example.invalid/a?page=2>; rel=\"next\"".into()); }
                Ok(response)
            })?;
            result.extend(a.as_array().unwrap().clone());
            if let Ok(b) = collect("https://example.invalid/b", |_| if fail { Err("temporary B failure".into()) } else { Ok(page(json!([10]))) }) { result.extend(b.as_array().unwrap().clone()); }
            Ok(result)
        };
        let first = composite_page("fixture-composite", None, || read(true)).unwrap(); assert_eq!(first.items, vec![json!(1)]);
        let cursor = first.cursor.unwrap();
        assert!(composite_page("different-binding", Some(&cursor), || read(false)).is_err());
        let second = composite_page("fixture-composite", Some(&cursor), || read(false)).unwrap(); assert_eq!(second.items, vec![json!(2),json!(10)]); assert!(second.cursor.is_none());
        let replay = composite_page("fixture-composite", Some(&cursor), || read(false)).unwrap(); assert_eq!(second.items, replay.items);
    }

    #[test]
    fn pages_arrive_independently_and_retry_is_bound_to_the_collection() {
        let mut calls = 0;
        let first = super::page("https://api.example/pulls", None, |_| { calls += 1; let mut p = page(json!([1,2])); p.link = Some("<https://api.example/pulls?page=2>; rel=\"next\"".into()); Ok(p) }).unwrap();
        assert_eq!(calls, 1); assert_eq!(first.items, json!([1,2]));
        let cursor = first.cursor.unwrap();
        assert!(super::page("https://api.example/other", Some(&cursor), |_| panic!("cursor crossed collections")).is_err());
        assert!(super::page("https://api.example/pulls", Some(&cursor), |_| Err("temporary failure".into())).is_err());
        let second = super::page("https://api.example/pulls", Some(&cursor), |url| { assert!(url.ends_with("page=2")); Ok(page(json!([3]))) }).unwrap();
        assert_eq!(second.items, json!([3])); assert!(second.cursor.is_none());
        assert!(super::page("https://api.example/pulls", None, |_| { let mut p = page(json!([])); p.link = Some("<https://foreign.example/leak>; rel=\"next\"".into()); Ok(p) }).is_err());
    }

    #[test]
    fn collects_all_pages_and_rejects_foreign_hosts_and_loops() {
        let mut calls=0;
        let result=collect("https://api.example/repos?per_page=2", |url| {
            calls+=1;
            Ok(if parameter(url, "page").as_deref() == Some("2") {page(json!([3]))} else {page(json!([1,2]))})
        }).unwrap();
        assert_eq!(result,json!([1,2,3]));assert_eq!(calls,2);
        let mut calls=0;
        assert!(collect("https://api.example/repos", |_| { calls+=1; Ok(page(json!({"values":[],"next":"https://evil.example/repos"}))) }).is_err());
        assert_eq!(calls,1);
        assert!(collect("https://api.example/repos", |_| Ok(page(json!({"values":[],"next":"https://api.example/repos"})))).is_err());
    }
    #[test]
    fn follows_server_offset_and_continuation_headers() {
        let mut calls=0;
        let value=collect("https://api.example/list?limit=1", |url| {
            calls+=1;
            Ok(if url.contains("start=1") {page(json!({"values":[2],"isLastPage":true}))} else {page(json!({"values":[1],"isLastPage":false,"nextPageStart":1}))})
        }).unwrap();assert_eq!(value["values"],json!([1,2]));assert_eq!(calls,2);
        let mut calls=0;
        let value=collect("https://api.example/list", |url| {
            calls+=1; let mut p=page(json!({"value":[calls]}));
            if calls==1 {p.continuation=Some("a+b".into());} else {assert!(url.contains("continuationToken=a%2Bb"));}
            Ok(p)
        }).unwrap();assert_eq!(value["value"],json!([1,2]));
    }
}
