# Hosted content policy

Pull request and issue descriptions, comments and reviews come from hosting services and are shown as Markdown by [markdown.rs](../crates/gitcraft/src/app/markdown.rs). They are untrusted, so GitCraft limits what they can do:

- Raw HTML in Markdown is left out, not rendered.
- Links open only `http` and `https` addresses without a user name or password, resolved against the page they came from, and only through the system browser when clicked.
- Pictures are shown as their alternative text. GitCraft does not download images named in hosted content.
- Author pictures come only from Gravatar (`gravatar.com`, over HTTPS), looked up by the SHA-256 hash of the commit author's address ([avatars.rs](../crates/gitcraft-core/src/avatars.rs)). Turning on **Display author initials and generic remote icons instead of avatars** in Preferences → UI Customization stops those requests.
- Check links from CI services open only over HTTPS.
- Hosting, AI and update requests are made by the back end; hosted content can't start requests of its own.

When you change [markdown.rs](../crates/gitcraft/src/app/markdown.rs) or the hosting views, keep its `only_plain_web_links_open` test passing and add a case for any new kind of link or content.
