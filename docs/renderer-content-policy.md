# Renderer content policy prototype

`tauri.csp.conf.json` is an opt-in packaged-build configuration. Build it with
`npm --prefix ui run build`, then `./ui/node_modules/.bin/tauri build --config tauri.csp.conf.json` from the app root.
The default policy remains unchanged until packaged validation passes.

Source inventory (2026-11-17):

- Vite bundles all renderer scripts, dynamic chunks, fonts and editor language grammars locally.
- Monaco creates a local editor worker through Vite's worker import. Blob workers are allowed for webview implementations that wrap worker URLs.
- React, Monaco and resizing/layout controls set inline styles. The policy allows inline styles; executable inline scripts require Tauri's generated hashes.
- Author images request HTTPS Gravatar URLs and may use Gravatar subdomains. Image previews are native-generated data URLs. No arbitrary image host is required.
- Renderer commands and events use Tauri IPC (`ipc:` and the IPC localhost endpoint). Git hosting, AI, downloads and updater HTTP requests execute natively, so they do not require renderer network permission.
- Links launch through native commands. Embedded frames, plugins, forms and injected base URLs are unnecessary.
- No eval permission is included. Development-server HMR is outside this packaged policy.

Before enabling this configuration by default, test an installed build on Windows,
Linux and macOS: cold launch; file/diff editors and language worker startup;
image previews and avatars; preferences; notifications; all dialogs; native
account requests; update discovery and restart. Record CSP violations and actual
worker/IPC origins for each platform. A browser harness cannot establish these
packaged webview results.

Windows release-executable validation now passes IPC, editor/JSON workers, data
images, the HTTPS avatar source, preferences and native updater configuration,
with zero recorded CSP violations or renderer exceptions. See
[continuation validation](continuation-validation-2026-11-17.md) and the repeatable
[Windows harness](../TESTING.md#windows-release-webview-regression-checks).
This does not establish signed update installation or installed macOS/Linux behavior.
