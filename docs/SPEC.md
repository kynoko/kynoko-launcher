# Kynoko Launcher - Specification

Status: **draft for review** (2026-09-25). Nothing here is implemented yet,
except the loopback bridge spike in `spikes/loopback-bridge/` (phase 0).

Kynoko Launcher is the desktop entry point to the Kynoko web apps on
Windows, macOS and Linux. It installs them, gives them shortcuts, opens the
user's files in them with the browser of the user's choice, lets the apps save
those files back in place, and removes every trace of itself when uninstalled.

Contents:

1. Goals and non-goals
2. Vocabulary
3. Architecture
4. The online catalogue
5. Host browsers and profiles
6. Shortcuts
7. File associations
8. The loopback bridge (normative protocol)
9. The Local Network Access prompt
10. The web side: contract with the Kynoko app skeleton
11. Install, uninstall, cleanup
12. Unsigned distribution
13. Self-update notice
14. Mobile
15. Internationalisation
16. Open questions
17. Phases
18. Decision log

---

## 1. Goals and non-goals

Goals:

- **One entry point on desktop**, for every Kynoko app, whatever the browser.
  The apps stop offering their own PWA install on desktop and point to
  Kynoko Launcher instead.
- **Any installed browser**, channel variants included (Firefox *or* Firefox
  Nightly, Chrome *or* Chrome Canary, Safari *or* Safari Technology Preview,
  LibreWolf, Brave, Opera, Vivaldi, Zen...). A default browser and profile,
  overridable per app.
- **Direct shortcuts** to every app, and to each app's façades in a submenu.
- **File associations** per extension, driven by what each façade declares;
  a double-click opens the file in the right façade.
- **Save in place**: the app writes back to the original file. Required from v1.
- **Full reversibility**: any association, shortcut or default it created can
  be removed on demand, and all of them are removed on uninstall, with the
  user's previous defaults restored.
- **Open source** (Apache-2.0), **unsigned** builds, published on GitHub under
  the `kynoko` account.

Non-goals:

- Embedding a browser. The host is always a browser the user installed.
- Changing browser settings on the user's behalf (enterprise policies,
  profile prefs). Browsers stay unmanaged.
- Silent PWA installation. Where a browser can install the app as a PWA, the
  user confirms it in the browser, once.
- iOS. See section 14.

## 2. Vocabulary

| Term | Meaning |
|---|---|
| App | A Kynoko web app with its own origin, e.g. `https://office.kynoko.com` (the "engine" in platform terms). |
| Façade | A tool inside an app, reached at a path of the app (`/document`, `/spreadsheet`...). Declared by the app in `assets/kynoko-app.json` (`tiles`). |
| Host browser | The browser (and profile) an app is opened with. |
| Catalogue | The public, read-only list of apps, façades and accepted file types that Kynoko Launcher follows. |
| Bridge | The loopback HTTP channel through which a page reads and writes one local file. |
| Session | One file opened through the bridge: a path, an allowed origin, a token. |
| Agent | The resident part of Kynoko Launcher that owns the bridge while files are open. |

## 3. Architecture

```
 double-click foo.docx
        |
        v
 OS association ──> kynoko-launcher open "C:\...\foo.docx"
                          |
                          |  1. extension -> app + façade (catalogue + user choices)
                          |  2. new bridge session (path, origin, token)
                          |  3. launch host browser of that app:
                          |     https://office.kynoko.com/open#kynokoBridge=127.0.0.1:<port>/<token>
                          v
                    Agent (127.0.0.1:<port>)  <── GET /content, PUT /content ──  the app's page
```

- **Desktop application**: Tauri 2 (Rust core + web UI). Rust gives direct
  access to the registry, LaunchServices and the XDG files, and Tauri's
  bundler produces the installers of the three systems.
- **One binary, several roles**: the settings window, the `open` / `launch`
  command-line entry points used by associations and shortcuts, and the agent.
  A second invocation hands its arguments to the running instance
  (single-instance), so only one agent owns the bridge.
- **The agent** runs while at least one session is open, then exits after an
  idle delay. It shows a tray / menu-bar icon while alive, so the user can see
  why it runs and close sessions.
- **Window UI**: `@common/kynoko-ui` is a private package and this public
  repository must build on GitHub Actions without private registry
  credentials. So only the **Boréal design tokens** (CSS custom properties:
  colours, spacing, radii, typography) are vendored here, and the few
  components the window needs are written in this repository. Vendored
  tokens become Apache-2.0 like the rest; the fonts keep their own licenses
  (Inter: SIL OFL 1.1); brand assets (logo, app icons) are not vendored, they
  come from the catalogue at run time and stay trademarks (TRADEMARKS.md).
- **State** lives in the user's config directory: settings, catalogue cache,
  and an **inventory** of everything written to the system (section 11).

## 4. The online catalogue

### Source

Each app already publishes `assets/kynoko-app.json` (`appCode`, `version`,
`locales`, `deepLinks`, `tiles` with `path` and `names`). The platform already
pulls it to sync façades (`backend/specifications/APP_FACADES.md`).

New in that manifest: each tile declares the files it opens.

```json
{
  "path": "document",
  "names": { "en": "Document", "fr": "Document" },
  "files": [
    { "ext": "docx", "mime": "application/vnd.openxmlformats-officedocument.wordprocessingml.document" },
    { "ext": "odt",  "mime": "application/vnd.oasis.opendocument.text" }
  ]
}
```

The platform aggregates the synced manifests into the **catalogue**, a
pre-generated static JSON document, so that serving it costs nothing:

```json
{
  "v": 1,
  "generatedAt": "2026-09-25T12:00:00Z",
  "apps": [
    {
      "code": "Office",
      "url": "https://office.kynoko.com/",
      "names": { "en": "Kynoko Office", "fr": "Kynoko Office" },
      "icon": { "png512": "https://.../icons/office.3f9a1c.png", "sha256": "3f9a1c..." },
      "status": "live",
      "facades": [
        {
          "path": "document",
          "names": { "en": "Document" },
          "icon": { "png512": "https://.../icons/office-document.8b2d.png", "sha256": "8b2d..." },
          "files": [ { "ext": "docx", "mime": "...", "primary": true } ]
        }
      ]
    }
  ]
}
```

- `primary` marks the façade the platform proposes by default when several
  façades or apps accept the same extension (e.g. `.png` in Photo Studio and
  Media Studio). The user can change it per extension (section 7).
- Icon URLs carry their content hash: an icon never changes at a given URL and
  is downloaded once.
- The exact URL of the catalogue is to be decided with the platform (it goes
  through `api.kynoko.com`, trailing slash included). It must be overridable in
  Kynoko Launcher's settings for the dev domains.

### Refresh policy

- The catalogue is refreshed **at most every 12 hours**, plus on demand
  ("Check now", itself limited to once per minute).
- Each request is conditional (`If-None-Match` / `If-Modified-Since`); a `304`
  costs the server nothing and updates only the "last checked" time.
- The first check after start is delayed by a **random jitter** (0-10 min), so
  that machines started at the same hour do not hit the server together.
- A failure retries with an exponential backoff (5 min, 15 min, 1 h, capped by
  the 12 h period).
- A failure is shown **only inside Kynoko Launcher**, as a quiet line with
  the date of the last successful update. **No system notification**, ever.
- Offline, or before the first success, the last cached catalogue is used, or
  the snapshot bundled in the build.

### Reconciliation

When a new catalogue arrives, Kynoko Launcher applies the difference:

| Change | Effect |
|---|---|
| New app | Listed as available; nothing installed without the user. |
| App removed or `status` not live | Its shortcuts and associations are removed; the user is told in the window. |
| New façade | Its shortcut is added in the app's submenu if the app is installed. |
| New extension on a façade | Associated if the app is installed and the user did not opt out of that extension. |
| Extension removed | Its association is removed and the previous default restored. |
| Names, icons | Shortcuts updated in place. |

**User choices always win**: an extension the user disabled, a façade shortcut
the user removed, an extension the user gave to another app, stay that way.

## 5. Host browsers and profiles

### Discovery

Browsers are discovered from what the system declares, never from a hard-coded
list, so every channel and fork appears by itself.

| OS | Source | Notes |
|---|---|---|
| Windows | `HKLM` and `HKCU` `SOFTWARE\Clients\StartMenuInternet\*` (+ `WOW6432Node`): name, icon, `shell\open\command` | Firefox Nightly, Chrome Canary etc. register separately. |
| macOS | `LSCopyApplicationURLsForURL` / `NSWorkspace.urlsForApplications(toOpen:)` on an `https` URL | Includes Safari Technology Preview. |
| Linux | `.desktop` files declaring `x-scheme-handler/https`, in the XDG data dirs, Flatpak exports and Snap | Flatpak/Snap browsers share the host network by default; to verify with the bridge. |

Each browser gets an **engine** (Chromium, Gecko, WebKit), from its
executable metadata / bundle identifier, which decides how it is launched.

### Profiles

The Kynoko session and the Local Network Access permission belong to a
browser **profile**, so the profile is part of the choice:

| Engine | Discovery | Launch |
|---|---|---|
| Chromium | `Local State` -> `profile.info_cache` | `--profile-directory=<dir>` |
| Gecko | `profiles.ini` / `installs.ini` | `-P <name>` (or `-profile <path>`) |
| WebKit (Safari) | none usable from the command line | the default profile only |

### Defaults and overrides

- A **default browser + profile** is chosen at first run (proposal: the
  system's default browser, its default profile).
- **Each app** follows the default unless it has its own browser + profile.
  Changing the default moves every app without an override.
- Before switching an app to another browser or profile, the window warns that
  the app will be there **without its session** (sign in again), **without its
  local data** (drafts, local storage) and will ask the Local Network Access
  question once more.
- If a configured browser disappears, the next launch says so and offers to
  pick another one; nothing falls back silently.

### Launch

| Engine | Command |
|---|---|
| Chromium | `<exe> --profile-directory=<dir> --app=<url>` (app window). If the app is installed as a PWA in that profile, launching it by `--app-id` at a given URL is to be verified in phase 1. |
| Gecko | `<exe> -P <profile> -new-window <url>` |
| WebKit | `open -a <Safari.app> <url>` |

## 6. Shortcuts

Every shortcut runs **Kynoko Launcher**, not the browser:
`kynoko-launcher launch <appCode>[/<façadePath>]`. The browser is resolved
at launch time, so changing browsers never rewrites shortcuts, and a removed
browser gives a clear message instead of a dead icon.

| OS | App | Façades (submenu) | Icons |
|---|---|---|---|
| Windows | `.lnk` in `Start Menu\Programs\<App name>\` (desktop shortcut optional) | `.lnk` in the same folder (one level: nested folders display poorly on Windows 11) | `.ico` generated from the catalogue PNG |
| macOS | a small shortcut `.app` in `~/Applications/Kynoko/<App name>/` | shortcut `.app` next to it | `.icns` |
| Linux | `.desktop` in `~/.local/share/applications/` | **desktop actions** of the app's `.desktop`: a real right-click submenu in GNOME and KDE | PNG in the hicolor theme |

- Shortcut `.app` bundles are created locally, so they carry no quarantine
  attribute and open without a Gatekeeper warning. They still need an ad hoc
  signature on Apple Silicon (section 12).
- By default: the app shortcut is created; façade shortcuts are offered as
  checkboxes.
- Later, not v1: façades in the Windows taskbar jump list.

## 7. File associations

### What is associated

For each installed app, each extension declared by its façades, unless the
user turned it off. When several façades or apps claim an extension, the one
marked `primary` is proposed and the user can pick another.

### Registration

| OS | Mechanism | Default handler |
|---|---|---|
| Windows | `HKCU\Software\Classes`: a ProgID `Kynoko.<App>.<ext>` (icon, verb `open` -> `kynoko-launcher open "%1"`), `.<ext>\OpenWithProgids`, plus `Capabilities` + `RegisteredApplications` so that Kynoko Launcher appears in Settings > Default apps | **Cannot be set programmatically** (hashed `UserChoice`). Kynoko Launcher opens the Default apps page on its own entry and explains. |
| macOS | Document types are **static**: the `Info.plist` of Kynoko Launcher declares every Kynoko type with `LSHandlerRank = Alternate`. Enabling an extension = making it the default | `NSWorkspace.setDefaultApplication(at:toOpenContentType:)` (may show a system confirmation) |
| Linux | `MimeType=` of a `kynoko-launcher-open.desktop`, a shared-mime-info XML for types the system lacks, `update-desktop-database` | `xdg-mime default` / `~/.config/mimeapps.list` |

On macOS, files arrive through Apple Events, not `argv` (Tauri:
`RunEvent::Opened`).

Consequence of the macOS static declaration: while Kynoko Launcher is
installed, it is listed in "Open With" for every Kynoko type, even the ones
turned off; turning an extension off only removes the default.

### Defaults: backup and restore

Before changing the default handler of an extension, the previous one is
recorded in the inventory. Removing the association, or uninstalling,
restores it. On Windows, where the default cannot be written, removing the
ProgID makes the system ask again on the next double-click, which is the
correct outcome.

### No `file_handlers` in the web manifests

The apps' web manifests must **not** declare `file_handlers` for desktop:
Chromium would register its own associations when a PWA is installed, which
Kynoko Launcher could neither list nor remove, and which would duplicate
its own entries. Exception, after v1: a ChromeOS-only manifest (section 10).

## 8. The loopback bridge (normative protocol)

Validated in phase 0 with Edge 154 and Firefox 156 (`spikes/loopback-bridge`).

### Listener

- `127.0.0.1` only, a random port chosen at agent start.
- Every request must carry `Host: 127.0.0.1:<port>` exactly (DNS rebinding),
  else `421`.
- Every request must carry an `Origin` equal to the session's allowed origin
  exactly (scheme, host, port), else `403` **without CORS headers**.
  A missing `Origin` is refused.
- The allowed origin is the app's origin from the catalogue (or the dev
  override). Never a wildcard.

### Session

- Created by Kynoko Launcher only, from the operating system (association,
  "Open" in its window). **A web page can never name a path.**
- Token: 256 random bits, compared in constant time, naming exactly one file.
- Passed to the page in the URL **fragment** (never sent to a server, never in
  a `Referer`): `#kynokoBridge=127.0.0.1:<port>/<token>`. The page removes it
  from the address bar at once (`history.replaceState`) and keeps it in
  `sessionStorage`, so a reload still works and the token does not end up in
  history or bookmarks.
- Several files: one session each, fragment `kynokoBridge=<host>/<t1>,<t2>`.
- Lifetime: kept alive by a heartbeat every 30 s; closed by the page, by the
  user from the tray, or after 10 min without heartbeat.

### Endpoints

| Method | Path | Result |
|---|---|---|
| `OPTIONS` | any | `204`, CORS preflight (methods `GET, PUT, POST, DELETE`; headers `If-Match, Content-Type`); `Access-Control-Allow-Private-Network: true` when asked |
| `GET` | `/s/<token>/meta` | `200` `{ name, size, etag }`; `410` if the file is gone |
| `GET` | `/s/<token>/content` | `200` streamed bytes, `ETag` exposed |
| `PUT` | `/s/<token>/content` | Requires `If-Match: <etag>`. `200` `{ etag }`; `412` + current `ETag` if the file changed on disk; `409` if it cannot be written (locked by another program) |
| `POST` | `/s/<token>/heartbeat` | `204` |
| `DELETE` | `/s/<token>` | `204`, session closed |
| other | | `404` (unknown token) / `405` |

Every response carries `Access-Control-Allow-Origin: <origin>`,
`Access-Control-Expose-Headers: ETag`, `Vary: Origin`, `Cache-Control: no-store`.

### Writing

- The body is streamed to a temporary file **in the same directory**, synced,
  then swapped with the original. A crash leaves the old or the new file,
  never a half-written one.
- The swap must preserve what a plain rename loses:
  - Windows: `ReplaceFileW` (keeps ACLs, attributes, alternate streams);
  - macOS: copy extended attributes and permissions to the temporary file first
    (Finder tags, labels), then rename;
  - Linux: copy mode, owner when possible, and extended attributes, then rename;
  - hard links are broken by any atomic replace: detect `nlink > 1` and fall
    back to an in-place write after a backup copy.
- The ETag is `"<mtime ns hex>-<size hex>"`. A change on disk between read and
  write gives `412`; the app must then offer: overwrite, save as a copy, or
  reload.

### Measured (phase 0, Windows 11)

256 MB read in 1.2 s (Edge) / 1.3 s (Firefox); write back, `412` on a stale
write, and foreign-origin refusal verified in both.

## 9. The Local Network Access prompt

Browsers gate requests from a public page to `127.0.0.1` behind a permission,
per origin and per profile. Measured in phase 0:

- **Chromium** (Edge 154): refused until the `loopbackNetwork` permission is
  granted (distinct from `localNetwork`).
- **Firefox** 156: prompt *"<origin> souhaite accéder à d'autres applications
  et services sur cet appareil"*, a **"Se souvenir de ce choix pour ce site"**
  checkbox, **unchecked by default**, and Autoriser / Bloquer. Unchecked, the
  permission is **temporary: 24 h** (`network.lna.temporary_permission_expire_time_ms`),
  surviving a restart. Checked, it is kept.
- Safari: not measured yet.

Decision: **accept the prompt** (one per app origin and profile). No
pre-granting through browser policies, no workaround.

The app shows its own short explanation **right before** the first bridge
request, adapted to the browser:

> **Autoriser l'ouverture de vos fichiers**
>
> Pour ouvrir et enregistrer ce fichier directement sur votre ordinateur,
> Kynoko Office passe par Kynoko Launcher. Votre navigateur va demander si
> ce site peut « accéder à d'autres applications et services sur cet
> appareil ».
>
> Cochez **« Se souvenir de ce choix pour ce site »**, puis cliquez sur
> **« Autoriser »**. Cette question ne sera plus posée.

On refusal, the app explains how to change the answer (Firefox: the device
icon left of the address) and offers to retry.

**The page never probes the bridge** to detect whether Kynoko Launcher is
installed: any such request would itself raise the prompt. The page only talks
to the bridge when it holds a session token.

## 10. The web side: contract with the Kynoko app skeleton

To be implemented in the skeleton (`kynoko-skeleton/frontend`) and documented
in `0-template`, then adopted by Office, Photo Studio and Media Studio.

- **Declaration**: façades declare `files` in `assets/kynoko-app.json`
  (section 4). The same declaration drives the in-app routing, the catalogue
  and Android's `share_target`, so they cannot drift.
- **`/open` route**: generic. Reads the file source, picks the façade from the
  extension, hands the file over (generalising Office's
  `file-handoff.service.ts` contract: IndexedDB `pending-import`, `?import=1`).
- **File source abstraction**: every opened document knows where it came from
  and how to save:

  | Source | Where | Save |
  |---|---|---|
  | `bridge` | Kynoko Launcher, any desktop browser | `PUT` with `If-Match`, in place |
  | `handle` | "Open" button with `showOpenFilePicker` (Chromium), ChromeOS `file_handlers` | `createWritable()`, in place |
  | `share` | Android `share_target` (service worker) | copy: save = download / share |
  | `picker` | `<input type=file>` everywhere else (Firefox/Safari without Kynoko Launcher, iOS) | copy: save = download |

  The editors call one `save()`; the source decides. A copy never pretends to
  be saved in place: the UI says "download" when it is one.
- **Conflict UI** for `412`: overwrite / save as copy / reload.
- **Pre-prompt** of section 9, shown once per origin and profile (remembered in
  `localStorage` after a successful bridge request).
- **Manifests**: no `file_handlers` on desktop. After v1, a ChromeOS manifest
  with `file_handlers`, served **only** when the platform is explicitly
  ChromeOS (`navigator.userAgentData.platform === 'Chrome OS'`), never by
  default.
- **Install invitation on desktop**: the "Install" banner and menu entry offer
  to download Kynoko Launcher for the detected OS, instead of the browser's
  PWA prompt. Mobile keeps the current PWA flows.

## 11. Install, uninstall, cleanup

### Packages

| OS | Package | Scope |
|---|---|---|
| Windows | NSIS installer from Tauri, **per-user**, no admin rights | `%LOCALAPPDATA%`, `HKCU` only |
| macOS | `.dmg` with `Kynoko Launcher.app` | `/Applications` or `~/Applications` |
| Linux | `.deb`, `.rpm`, AppImage | user-level integration in `~/.local` and `~/.config` |

### Inventory

Every artefact written to the system is recorded **before** being written:
registry keys and values, `.desktop` and MIME files, shortcut bundles and
`.lnk`, icons, and every previous default replaced. Cleanup walks the
inventory, so it removes exactly what was created, and nothing else.

### Uninstall = cleanup

1. Restore the recorded previous defaults.
2. Remove associations (ProgIDs, `.desktop` MIME entries), shortcuts and
   shortcut bundles, icons.
3. Remove its own state (settings, catalogue cache, inventory).

The same cleanup is available as a button ("Remove everything") and as
`kynoko-launcher cleanup`.

What each system allows:

- **Windows**: the NSIS uninstaller runs `cleanup` before removing files.
  Complete.
- **macOS**: there is no uninstaller; dragging the app to the Trash runs no
  code. The window offers **"Uninstall Kynoko Launcher"**, which cleans up
  then moves the app to the Trash. If the app is trashed directly, LaunchServices
  forgets its document types by itself, but shortcut bundles and changed
  defaults remain: the documentation says so, and a reinstall followed by
  "Uninstall" finishes the job.
- **Linux**: package removal scripts run as root and must not touch users'
  home directories. `.deb` / `.rpm` removal leaves the per-user integration;
  the window's "Uninstall" and `kynoko-launcher cleanup` do the complete
  job, and the documentation says to run them first. The AppImage has the same
  "Uninstall" entry.

Browser-side data (sessions, local storage, LNA permissions, PWAs the user
installed in a browser) belongs to the browser and is not touched; the
documentation explains how to remove it.

## 12. Unsigned distribution

No paid signing identity. Consequences, documented on the download page:

- **Windows**: SmartScreen warns ("More info" > "Run anyway").
  **Smart App Control**, enabled on some clean Windows 11 installs, blocks
  unsigned programs outright; the only way round is turning it off, which the
  documentation must state plainly.
- **macOS**: Gatekeeper blocks the first launch; since macOS 15 the user must go
  to System Settings > Privacy & Security > "Open Anyway". Apple Silicon
  requires an **ad hoc** signature (`codesign -s -`, free, no identity), which
  the build applies.
- **Linux**: no restriction.

Trust instead of signatures: public source, SHA-256 checksums published with
every release, builds from GitHub Actions visible to all, and reproducible
builds as a goal.

## 13. Self-update notice

No auto-update (Tauri's updater requires its own signing key; to revisit).
Kynoko Launcher checks the latest GitHub release with the same policy as
the catalogue (12 h, conditional, jitter) and shows a line in its window when a
newer version exists. No system notification.

## 14. Mobile

- **Android**: phase 2, an open source companion APK (self-generated signing
  key, F-Droid or direct download), **in this repository**: Tauri 2 builds
  Android from the same Rust core (catalogue, bridge, guards), so the logic is
  written once. It registers in "Open with" for the Kynoko
  types, keeps write access to the document (`takePersistableUriPermission`)
  and serves it through the **same bridge protocol**, so the apps need nothing
  more. To verify then: Chrome Android's Local Network Access behaviour and
  background limits. Until then, the existing `share_target` keeps working
  (copy only).
- **iOS**: no companion is possible without the App Store (paid account,
  signing). Safari gives web apps neither share target nor "Open with". The
  apps offer their file picker, and saving is a download or the share sheet to
  Files. The documentation says so plainly.

## 15. Internationalisation

Like every Kynoko app: fr, en, es, ar, ja, zh-Hans, zh-Hant from the start,
right-to-left layout for Arabic (logical CSS), technical fields (paths,
extensions, URLs) kept left-to-right. Shortcut and façade names come from the
catalogue in the system's language, falling back to English.

## 16. Open questions

1. ~~UI toolkit~~ decided: see *Window UI* in section 3.
2. **Safari**: does a public HTTPS page reach `http://127.0.0.1`, and with which
   prompt? Needs a Mac.
3. **Edge / Chrome prompt**: wording and "remember" behaviour, headed test
   (`node lna-manual.mjs edge`).
4. **Chromium installed PWAs**: can Kynoko Launcher launch an installed PWA
   at a given URL (`--app-id` with a URL), or is `--app=<url>` the only
   reliable form?
5. **Catalogue URL** and generation job on the platform side.
6. **Linux sandboxed browsers** (Flatpak, Snap): loopback reachable, launching
   with a profile.

## 17. Phases

| Phase | Content |
|---|---|
| 0 - done | Bridge spike: protocol, guards, write back, LNA measured (Edge, Firefox). |
| 1 | Skeleton: `files` declaration, file source abstraction with `bridge` and `handle`, generic `/open`, save in place, conflict UI, pre-prompt, no `file_handlers`, desktop install invitation. Office, Photo Studio, Media Studio. |
| 2 | Kynoko Launcher v1: catalogue, browsers and profiles, shortcuts, associations, agent, inventory and cleanup, i18n. **Windows slice done** (2026-09-25): bundled catalogue, browser discovery (every channel), per-app browser, HKCU associations with inventory and complete removal, `open` / `launch` / `cleanup`, single-instance agent with the multi-session bridge (ReplaceFileW), NSIS per-user installer running `cleanup` on uninstall, 7-language window. Verified end to end: associate Office, double-click a .docx, edit, Ctrl+S on disk, remove everything, registry back to its prior state. Next: shortcuts, online catalogue refresh, profiles, macOS, Linux. |
| 3 | Packaging on GitHub Actions for the three systems, checksums, download page and per-OS documentation. |
| 4 | Android companion APK. ChromeOS manifest. |

## 18. Decision log

| Date | Decision |
|---|---|
| 2026-09-25 | Kynoko Launcher is the single desktop entry point, Chromium included; `file_handlers` removed from web manifests. |
| 2026-09-25 | Any installed browser, channel variants included; default browser + profile, overridable per app. |
| 2026-09-25 | Save in place required from v1. |
| 2026-09-25 | Open source, Apache-2.0 with a trademark notice, unsigned, GitHub `kynoko` account. |
| 2026-09-25 | Accept the Local Network Access prompt (option A), preceded by an in-app explanation. |
| 2026-09-25 | Catalogue public, refreshed every 12 h + manual, failures shown only in the window. |
| 2026-09-25 | Façade shortcuts in a submenu of their app. |
| 2026-09-25 | ChromeOS kept (after v1); Android companion in phase 2; iOS limits documented honestly. |
| 2026-09-25 | Window UI: vendor the Boréal design tokens only, no dependency on the private kynoko-ui package. |
| 2026-09-25 | The Android companion lives in this repository (Tauri 2 mobile, shared Rust core). |
| 2026-09-26 | The window's menu entry lives in every app (skeleton v0.88.0): the launcher announces itself with a `kynokoLauncher=1` fragment marker and opens through `kynoko-launcher://settings?app=<code>`. |
| 2026-09-26 | Shortcuts: a Start menu folder per app with its listed facades; icons from the apps' manifests at run time. Browser profiles by default and per app. |
| 2026-09-26 | Online catalogue served by the platform (`GET /api/public/launcher-catalogue/`, ETag, 304); names from the platform's own bundles; draft facades not listed. |
| 2026-09-26 | Official builds fetch the Kynoko icon at release time (the repository keeps a neutral one); the bundled catalogue is refreshed from the platform at release time. |
| 2026-09-26 | macOS: document types declared in the bundle (`Info.mac.plist`, generated from the bundled catalogue, rank Alternate) and made default through LaunchServices on request; files and links arrive as Apple Events; shortcuts are small `.app` bundles in `~/Applications/Kynoko/`. |
| 2026-09-25 | Product renamed **Kynoko Launcher** (was "Kynoko Applications", too easily confused with the apps themselves); repository `kynoko/kynoko-launcher`, binary and packages `kynoko-launcher`. |
