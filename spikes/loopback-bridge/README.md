# Spike: loopback bridge (phase 0)

Can a web app, in ANY browser, read a local file handed over by Kynoko
Launcher and write it back in place, safely?

- `src/main.rs`: the bridge. One process = one file, one origin, one token.
  Guards: 127.0.0.1 only, exact `Host`, exact `Origin`, 256-bit token,
  `If-Match` on write (412 when the file changed on disk), atomic
  temp+rename, idle timeout.
- `page/open.html`: the web app's side (read, write back, conflict, close).
- `harness.mjs`: guards + full flow + foreign origin, headless Edge and Firefox.
- `lna.mjs`: the same flow from a page the browser believes is PUBLIC.
- `lna-debug.mjs`: CDP network log of the Edge case.

Run: `cargo build --release`, then `node harness.mjs 256`, `node lna.mjs`.

## Results (2026-09-25, Windows 11, Edge 154, Firefox 156)

| Case | Edge | Firefox |
|---|---|---|
| Guards (bad Host 421, bad/no Origin 403, bad token 404) | ok | ok |
| Page on an allowed origin: read 256 MB | ok, 1.2 s | ok, 1.3 s |
| Write back in place, stale write refused (412), reread | ok | ok |
| Page on another origin | blocked | blocked |
| Page on a PUBLIC origin, nobody answers | refused (`LocalNetworkAccessPermissionDenied`) | request held (permission prompt) |
| Page on a PUBLIC origin, permission granted | ok (`loopbackNetwork`) | ok (`network.lna.blocking=false`) |

Conclusion: the channel works, but a public page reaching 127.0.0.1 is gated
by Local Network Access in both engines: ONE permission prompt per app origin.
Chromium names it `loopbackNetwork` (distinct from `localNetwork`).

Not covered yet: Safari (needs a Mac), a headed run to see the prompts'
wording and whether Firefox remembers the answer, Linux/macOS builds.

Testing switches used to fake a public page:
- Chromium: `--ip-address-space-overrides=127.0.0.1:<port>=public`
- Firefox: `network.lna.address_space.public.override = 127.0.0.1:<port>`
