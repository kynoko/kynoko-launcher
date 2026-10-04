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
- `browsers` (per app, from the app's manifest since skeleton 0.93): the web
  engines it recommends and, for the others, what the user will miss, one
  sentence per limitation in every language:
  `{ "recommended": ["chromium"], "limitations": [{ "engines": ["gecko"], "texts": { "fr": "...", "en": "..." } }] }`.
  Engines are `chromium` (Chrome, Edge, Brave; the Kynoko window on Windows),
  `gecko` (Firefox) and `webkit` (Safari; the Kynoko window on macOS and
  Linux). `recommended` may also name `kynoko`: the Kynoko window itself,
  whatever its engine (skeleton 0.94). A limitation marked
  `"browsersOnly": true` is one the Kynoko window of that engine does not have,
  because the launcher provides it (the computer's fonts, section 10); the
  others stay true of it, even when the app recommends it (WebKit's, on macOS
  and Linux). The window marks the recommended browsers in each app's browser
  choice, and shows the limitations of the browser the app opens in under the
  app's line. A change of it rewrites no association and no shortcut.

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
- **Taskbar identity (Windows)**: an app's windows and its Start menu
  entries carry the app's own application identity (AppUserModelID
  `Kynoko.App.<code>`), not the launcher's. Without it, Windows draws every
  Kynoko window with the launcher's icon under the launcher's single button.
  With it, each app has its button (Office's windows together, apart from
  Photo Studio's), drawn with the app's icon; pinning it pins the app
  (relaunch command `kynoko-launcher launch <code>`, name "Kynoko <App>",
  the app's icon as an `.ico` in the window icons' cache), and a pinned
  Start menu entry and the windows it opens are one button. The window gets
  its identity before it is first shown. macOS has one Dock icon per
  program: nothing to do there; Linux: the window's icon only.

## 7. File associations

### What is associated

For each installed app, each extension declared by its façades, unless the
user turned it off. When several façades or apps claim an extension, the one
marked `primary` is proposed and the user can pick another.

### Registration

| OS | Mechanism | Default handler |
|---|---|---|
| Windows | `HKCU\Software\Classes`: a ProgID `Kynoko.<App>.<ext>` (the facade's icon, verb `open` -> the app's own program `open-with\kynoko-<app>.exe open --app <App> "%1"`, `Application\ApplicationName` + `ApplicationIcon` for its "Open with" entry), `.<ext>\OpenWithProgids`, plus `Capabilities` + `RegisteredApplications` so that Kynoko Launcher appears in Settings > Default apps | **Cannot be set programmatically** (hashed `UserChoice`). Kynoko Launcher opens the Default apps page on its own entry and explains. |
| macOS | Document types are **static**: the `Info.plist` of Kynoko Launcher declares every Kynoko type with `LSHandlerRank = Alternate`. Enabling an extension = making it the default | `NSWorkspace.setDefaultApplication(at:toOpenContentType:)` (may show a system confirmation) |
| Linux | `MimeType=` of one desktop entry per facade (`kynoko-launcher-open-<App>-<facade>.desktop`, named after the app, the facade's icon), a shared-mime-info XML for types the system lacks, `update-desktop-database` | `xdg-mime default` / `~/.config/mimeapps.list` |

On macOS, files arrive through Apple Events, not `argv` (Tauri:
`RunEvent::Opened`).

"Open with" names the app, with the brand once: "Kynoko Office", "Kynoko
Media Studio". Windows lists the entries of a type by program, merging the
types of the apps that share one, and names an entry after its program
unless the ProgID's `Application` key names it: each associated app gets
its own program, `open-with\kynoko-<app>.exe` next to the launcher (one
copy of the launcher's binary, the other apps' programs hard links to it,
as Chrome does for the web apps it installs). Started, it hands its
arguments to the launcher and quits at once, so it never holds a file an
update replaces; a new version refreshes it on its first start. macOS lists
Kynoko Launcher itself, its one bundle declaring every type.

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
| `PUT` | `/s/<token>/content` | Requires `If-Match: <etag>`. `200` `{ etag }`; `412` + current `ETag` if the file changed on disk; `409` `{ reason, holders, detail }` if it cannot be written: `reason` is `locked` (another program holds it; `holders` names the programs, from the Windows Restart Manager), `readonly`, `denied` (permissions, a protected folder) or `failed`; `detail` is the system's message. Launchers before 0.2.18 answered a plain-text `409` for all of these |
| `POST` | `/s/<token>/copy?suffix=<word>` | The body written BESIDE the file, as `<stem> (<word>)<ext>` (`word`: the page's own for "copy", kept to letters, digits and spaces), and to that same copy again on the next call of the session. `200` `{ name, path }` (the full path, for the page to say where the work went); `409` as for `PUT`. Launchers before 0.2.18 answer `405` |
| `POST` | `/s/<token>/reveal?which=file\|copy` | The file or its copy shown selected in the system's file manager (Explorer `/select`, Finder `open -R`, freedesktop `FileManager1.ShowItems` else the folder). `204`; `404` when there is no copy. Only these two paths: a session names no other |
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
- Windows: `ReplaceFileW` is given a BACKUP name (`.<name>.<random>.kynoko-old`,
  deleted after the swap). Without one, its error 1176 means "the original is
  deleted and the new content is still under the temporary name", which the
  cleanup then deleted too; with one, 1175 and 1176 leave both files where
  they were, and 1177 leaves the original under the backup's name, from where
  it is renamed back.
- A lock (sharing or lock violation, `ReplaceFileW` 1175-1177) is tried again
  for about two seconds (100, 200, 400, 600, 800 ms) before the write is
  refused: an antivirus or the indexer reading the file just written, a sync
  client, Explorer's preview pane hold a file only briefly.

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
- **The computer's fonts** (skeleton 0.94, `KynokoSystemFontsService`): a page
  cannot list them by itself (the list tells computers apart). In a Kynoko
  window, the launcher's `system_fonts` command answers, on every system:
  family names with their weights, italic and monospace, which is all a font
  menu needs (the engine finds a family by its name). In a Chromium browser,
  `queryLocalFonts()` answers after the browser's own permission prompt.
  Firefox and Safari have neither. The list stays in the page: it is never
  sent to a server.

  An app that draws text itself (a layout app shapes its lines with HarfBuzz
  and embeds the glyphs it used in the PDF it exports) needs a font's file,
  not its name. In a Kynoko window, `system_font_face` (`name`, `weight`,
  `italic`) picks the installed face CSS would draw for that family (a name
  `system_fonts` gives, compared without case), among the faces that are
  files on disk: the normal width first, then the style asked, then the
  nearest weight the CSS way. It answers `{ id, index }`, or `null` when there
  is no such face: `id` stands for the face during this run of the launcher,
  `index` is the face's index in its file (non-zero in a `.ttc` collection).
  `system_font_file` (`id`) answers the bytes of that face's file, as the
  response's raw body (an `ArrayBuffer`), read off the window's thread,
  128 MiB at most (CJK collections weigh tens of MB). Never a path, never
  another file: the page can only name a face the launcher found among the
  computer's fonts, and a file that is no longer a font is refused. The font
  stays licensed to the computer: the app may draw with it and embed it in a
  document it exports, and honouring the font's embedding permissions
  (OS/2 `fsType`) when it does is the app's responsibility, which the
  launcher cannot check.
- **The window's icon** (skeleton 0.98, `KynokoFaviconService`): a Kynoko
  window has no tab, and what the system draws for it (taskbar, Alt+Tab) is
  the window's own icon. The launcher opens it wearing the app's icon (its
  web manifest's) and, on Windows, the app's own taskbar identity
  (section 6). On a facade, the page hands its window the tab's drawing (the
  facade's icon with the app's seal) through `own_window_set_icon`, a PNG
  sent as the request's raw body, checked (a PNG of 16 to 1024 px, 1 MB at
  most) and put on the calling window only; leaving the facade, it hands the
  app's icon back.
- **Unsaved work** (skeleton 0.99, `KynokoUnsavedWorkService`): while a page
  holds work a close would lose, it says so (`own_window_guard`, true or
  false; a new page starts with none). The window's system close (Alt+F4,
  the taskbar, its system menu) is then held, and the page is sent the event
  `kynoko-close-requested`: it answers at once (`own_window_close_ack`) and
  asks its user "Save / Don't save / Cancel", then closes its window
  (`own_window_close`, which goes through) or keeps it. A page that does not
  answer within 2 s (frozen, or an app from before the guard) does not keep
  its window open. The bar's own cross asks the same question before it
  closes.

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
| 2026-09-27 | Each file type wears the name and icon of the facade it opens in (the one marking it primary), and its command names the app (`open --app <code>`). The user can take single types out of an app (exclusions, so types the app gains later still follow the app switch). A new version writes the associations again on its first start. Uninstall keeps its cleanup, except when a newer installer runs the uninstaller in place to replace the version. |
| 2026-09-27 | Uninstall always removes what was written to the system, but keeps the user's choices unless "Delete the application data" is ticked (Tauri's checkbox, unticked by default); a reinstall applies them again on its first start. Installer in the 7 Kynoko languages. "Select all" / "Deselect all" per app for file types. |
| 2026-09-27 | Installer wording: the page shown over an existing install says the preferences are kept whatever the choice ("Replace the old version (recommended)" / "Install over the old version"), and the uninstaller's box reads "Also delete my preferences". Tauri's language files, regenerated by tools/nsis-strings.py. Window labels name the result: "Open these files with a double-click", "Add to the Start menu" (Applications folder, applications menu). |
| 2026-09-27 | Firefox on Windows opens apps in a Taskbar Tab (its app window) through a gateway, https://launch.kynoko.com/: a Taskbar Tab always starts at its site's root, so the launcher leaves the real address (facade, file bridge) on a fixed loopback port (47318-47320, one-shot, gateway origin only) and the gateway navigates there, which stays in app mode under the same base domain. Until a browser profile has been seen reaching the launcher (gateway, and the app for a file), an ordinary window is used, where Firefox's permission prompt is sure to show. "The system's browser" is resolved to the installed browser it is, so a Chromium default gets its app window too. |
| 2026-09-27 | "Kynoko Launcher window" among the browsers: the app in the launcher's own window (WebView2, WKWebView, WebKitGTK), an app window on every system, files included (verified on Windows: a CSV double-clicked into Office's spreadsheet through the bridge). No capability for these windows (remote pages never reach the launcher's commands); navigation kept to kynoko.com and stripe.com, anything else goes to the system's browser; its own Kynoko session. Opera gets a plain window until --app is verified there. KYNOKO_LAUNCHER_HOME runs the launcher isolated, for end-to-end tests on a machine where it is installed. |
| 2026-09-27 | Installed over an existing version (same or older), the Windows installer offers "Update" (or "Reinstall"), keeping the preferences, or "Uninstall", a real uninstall that ends the installer; Tauri's two ways of installing are gone from that page. Tauri's own template and language files, fetched at the CLI's version and changed by tools/nsis-installer.py. Installing an older version keeps Tauri's page. |
| 2026-09-28 | Shortcuts in ONE Kynoko place: Windows Start menu folder "Kynoko" holding "<App>" and "<App> - <Facade>" (the Start menu shows a single level of folders, so no deeper tree); macOS Applications > Kynoko > <App> > entries; Linux one desktop entry per item under a "Kynoko" merged submenu (category X-Kynoko). One checkbox per entry (the app, each listed facade), exclusions kept in settings (excludedShortcuts). Each entry is recorded with its app (Artefact::Shortcut) so one app's entries go without touching the others'; the shared folder goes with the last. The per-app folders of 0.2.6 and before are moved on the first start of the new version. |
| 2026-09-28 | The window, redesigned with the design system's look (vendored tokens, faceted corners mirrored in right-to-left): the default browser on one line; each app a closed accordion whose line holds the app (click opens it), its browser (aligned across apps) and a stacked summary of its file types and shortcuts; inside, a switch "Associate the file types" with the types shown only while it is on, and the shortcuts as an accordion, one line each. No separate shortcuts switch: an app has shortcuts while one is ticked. |
| 2026-09-28 | Kynoko windows have no system title bar: the app's bar is the title bar (skeleton 0.91, KynokoNativeWindowService: its empty parts move the window, a double press maximizes it, window buttons at its end; macOS keeps its own over the bar's start). The page may only act on ITS OWN window, through own_window_* commands that take no target: Tauri's generic window commands would let it name another window (verified: they are refused, as are the launcher's commands). Every command is now listed in the app manifest, the settings window granted them explicitly. The window takes the page's title (taskbar, Alt+Tab). Files dropped on it reach the page. An isolated run skips the single-instance lock. |
| 2026-09-28 | A Kynoko window never depends on its page to be movable and closable: a page that has not taken the title bar over (own_window_* call) 2.5 s after loading gets the system's title bar back (an older app version served by its service worker, the sign-in page, a payment page, an error page), and a page that takes it removes it again. The window opened for a file comes to the front with the focus: the instance a double-click starts lends the running launcher the right to bring windows forward (AllowSetForegroundWindow) before handing the file over. |
| 2026-09-28 | "Open with" entries named after the app, the brand once ("Kynoko Office", "Kynoko Media Studio"), with the facade's icon. Windows: each associated app has its own program (`open-with\kynoko-<app>.exe`, a copy of the launcher's binary or a hard link to one) that hands over to the launcher at once, since Windows merges the entries sharing a program and names them after it; the ProgID's `Application\ApplicationName` / `ApplicationIcon` give the name and the icon (checked with SHAssocEnumHandlers, the list the "Open with" menu shows). A re-registration keeps the program instead of copying it again. Linux: the open entries named the same, the facade in their comment. macOS unchanged: one bundle, "Kynoko Launcher". |
| 2026-09-29 | Each app says, in its manifest, the browser engines it recommends and what the user misses in the others (skeleton 0.93, relayed by the platform's catalogue): the window marks "(recommended)" in the app's browser choice and shows the limitations of the browser the app opens in (its own, the default, or the system's) under the app's line. The Kynoko window counts as Chromium on Windows (WebView2), WebKit on macOS and Linux. |
| 2026-09-30 | The computer's fonts for the apps: a Kynoko window's page may call `system_fonts` (capability "kynoko-window", Kynoko origins only), which lists the families installed on the computer (fontdb, MIT: Windows' system and per-user folders, macOS' Library folders, fontconfig on Linux; ~60 ms for 750 faces, once per run, off the window's thread). Names, weights, italic and monospace only, never a font file: the engine draws a family from its name, and nothing copies a font that is licensed to the computer. Names follow the typographic family, as CSS and `queryLocalFonts()` do (Segoe UI Semibold is Segoe UI at 600), so a document names its fonts the same way in Chrome and in a Kynoko window. An app may recommend the Kynoko window itself (`kynoko`, Office first) and mark a limitation `browsersOnly` (what the launcher gives the Kynoko window of that engine). |
| 2026-09-30 | A refused save says why and where the work went: the bridge answers a `409` with the reason (locked, read-only, denied, failed), the programs holding the file (Windows Restart Manager) and the system's message; it waits out brief locks (about two seconds); `POST copy` writes the work beside the file (`<name> (<word>).<ext>`, the same copy on each refusal of the session) and answers its full path; `POST reveal` shows the file or the copy selected in the file manager. `ReplaceFileW` now gets a backup name, which makes each of its failures recoverable (before, a 1176 could lose the new content). |
| 2026-10-01 | A Kynoko window wears its app, not the launcher: it opens (hidden, then shown once dressed) with the app's icon and, on Windows, the app's own AppUserModelID with relaunch command, name and icon, so that each app has its own taskbar button and pins as itself; the app's Start menu entries carry the same identity. Tauri sets a window's small icon only, and the taskbar draws the big one: the launcher sets that one too (from the PNG, which Windows reads as an icon). The page puts the facade's drawing on its own window with `own_window_set_icon` (capability "kynoko-window"; a checked PNG as the raw body), the same picture as its tab. |
| 2026-10-01 | Closing a Kynoko window no longer loses unsaved work: a page holding some says so (`own_window_guard`), and the window's system close (Alt+F4, the taskbar) is then held while the page asks its user "Save / Don't save / Cancel" (event `kynoko-close-requested`, answered at once with `own_window_close_ack`; the page closes its window itself). The page's answer is awaited 2 s at most: a frozen page, or an app from before the guard, never keeps its window open. A page that loads anew starts unguarded. |
| 2026-10-03 | The title bar's hand-back no longer crosses a page's claim: the system's bar given back to a page that did not take the app's (TITLE_BAR_GRACE after its load) is decided and applied on the window's own thread, and a claim takes the bar away on that thread too, so that whichever comes first, a claim always wins. Checked on a helper thread and applied later, a claim landing in between was overwritten, and the window wore both bars. The skeleton (0.99.1) also claims again after the page's load and whenever the window comes to the front, and keeps asking while the launcher does not answer, which a launcher from before this fix needs. |
| 2026-10-04 | A Kynoko page may read the FILE of an installed font, not only its name: an app that draws text itself (shaping with HarfBuzz, embedding the glyphs it used in the PDF it exports) cannot do it with a name. `system_font_face` (family, weight, italic) picks the installed face the way CSS does (normal width, then the style, then the nearest weight), among the faces that are files on disk, and answers a number that stands for it during this run and its index in its file (`.ttc` collections); `system_font_file` reads that face's file with the number (raw bytes, 128 MiB at most, off the window's thread, refused if no longer a font). Both share the one scan of `system_fonts`, and are granted in the "kynoko-window" capability only (Kynoko origins, app windows). Never a path, never another file. The font stays licensed to the computer: honouring its embedding permissions (OS/2 fsType) when exporting is the app's responsibility. This replaces the "never a font file" of 2026-09-30 with "never a path, never another file". |
| 2026-09-25 | Product renamed **Kynoko Launcher** (was "Kynoko Applications", too easily confused with the apps themselves); repository `kynoko/kynoko-launcher`, binary and packages `kynoko-launcher`. |
