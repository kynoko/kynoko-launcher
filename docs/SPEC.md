# Kynoko Launcher - Specification

Status: **living specification**. Phases 0 to 3 are released (v0.2.x, see
section 17); each section describes the released behaviour unless it says
otherwise, and section 18 records why it changed.

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
19. Assistants (MCP): normative protocol

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
- **The agent** runs while it has something to do, then exits: the settings
  window shown, a Kynoko window open (and 40 s after the last one closes, the
  time the web engine takes to write its cookies to disk), a bridge session
  alive, a file just handed over whose page has not reached the bridge yet
  (120 s), or an install under way. It has no tray / menu-bar icon: one was
  planned here and never built. It will come with a resident mode, when a
  feature needs the agent to stay up.
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

- The catalogue is refreshed **at most every 12 hours**, plus when the
  window shows (at most once an hour) and on demand ("Check now"), these two
  limited to once per minute.
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
| New app | Gets its shortcuts (the app and its listed façades), like every app at the first start; its file types wait for its switch. An app the user took out of the menu is never put back (`knownApps`). |
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
- **What each app recommends, by default**: when the launcher first meets an
  app (the first start, a new app in the catalogue), if the browser it would
  follow (the default, else the system's: Windows' UserChoice, Linux'
  `xdg-settings`) is not one it recommends, it gets the first one it
  recommends that is installed (the Kynoko window for `kynoko`; for an
  engine, the usual stable browser of it, one opening app windows before
  Opera). Nothing recommended installed: it follows the default, and the
  window suggests one to install. Apps met before keep their choice.
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
- First start (nothing chosen yet): every app gets its shortcuts, the app
  and each listed façade, in the Kynoko place, named in the window's
  language; each can be unticked. A launcher already set up by hand (some
  app associated or given shortcuts) keeps its choices, and "Reset
  everything" does not bring them back.
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

On the first start the window asks once, above everything else: "Open your
files with the Kynoko apps?", every app with file types ticked and its types
listed under its name, so that what the answer replaces is in view.
"Associate" registers the ticked apps; "Later" leaves every app's switch off.
Nothing takes a file type over without that answer or the app's own switch.

### Registration

| OS | Mechanism | Default handler |
|---|---|---|
| Windows | `HKCU\Software\Classes`: a ProgID `Kynoko.<App>.<ext>` (the facade's icon dressed as a Kynoko file, see the 2026-10-05 entry, verb `open` -> the app's own program `open-with\kynoko-<app>.exe open --app <App> "%1"`, `Application\ApplicationName` + `ApplicationIcon` for its "Open with" entry), `.<ext>\OpenWithProgids`, plus `Capabilities` + `RegisteredApplications` so that Kynoko Launcher appears in Settings > Default apps | **Cannot be set programmatically** (hashed `UserChoice`). Kynoko Launcher opens the Default apps page on its own entry and explains. |
| macOS | Document types are **static**: the `Info.plist` of Kynoko Launcher declares every Kynoko type with `LSHandlerRank = Alternate`. Enabling an extension = making it the default | `NSWorkspace.setDefaultApplication(at:toOpenContentType:)` (may show a system confirmation) |
| Linux | `MimeType=` of one desktop entry per facade (`kynoko-launcher-open-<App>-<facade>.desktop`, named after the app, the facade's icon dressed as a Kynoko file), a shared-mime-info XML for types the system lacks, `update-desktop-database` | `xdg-mime default` / `~/.config/mimeapps.list` |

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
  "Open" in its window), or from the user's choice in the system's "Save as"
  dialog (`save-as`). **A web page can never name a path.**
- Token: 256 random bits, compared in constant time, naming exactly one file.
- Passed to the page in the URL **fragment** (never sent to a server, never in
  a `Referer`): `#kynokoBridge=127.0.0.1:<port>/<token>`. The page removes it
  from the address bar at once (`history.replaceState`) and keeps it in
  `sessionStorage`, so a reload still works and the token does not end up in
  history or bookmarks.
- Several files: one session each, fragment `kynokoBridge=<host>/<t1>,<t2>`.
- Lifetime: kept alive by a heartbeat every 30 s; closed by the page, or
  after 10 min without heartbeat.

### Endpoints

| Method | Path | Result |
|---|---|---|
| `OPTIONS` | any | `204`, CORS preflight (methods `GET, PUT, POST, DELETE`; headers `If-Match, Content-Type, Range`); `Access-Control-Allow-Private-Network: true` when asked |
| `GET` | `/s/<token>/meta` | `200` `{ name, size, etag, features }`; `410` if the file is gone. `features` lists what the launcher offers besides whole reads and writes: `range`, `append`, `save-as` (absent before 0.2.23, which offers none of them) |
| `GET` | `/s/<token>/content` | `200` streamed bytes, with `ETag`, `Content-Length` and `Accept-Ranges: bytes`. With one `Range: bytes=a-b`, `bytes=a-` or `bytes=-n` (the last n bytes): `206` with that slice only, streamed from disk, and `Content-Range: bytes a-b/<size>`; `416` with `Content-Range: bytes */<size>` when it holds nothing of the file (a start at or past its end, `bytes=-0`, an empty file). Several ranges, another unit or a malformed range: `200`, the whole file. Every answer carries the file's `ETag`: a page reading a file in parts compares it with the one it started from (no `If-Range`). Launchers before 0.2.23 ignore `Range` and answer `200` whole: a page checks for `206` (or `features`) before relying on it |
| `PUT` | `/s/<token>/content` | Requires `If-Match: <etag>`. `200` `{ etag }`; `412` + current `ETag` if the file changed on disk; `409` `{ reason, holders, detail }` if it cannot be written: `reason` is `locked` (another program holds it; `holders` names the programs, from the Windows Restart Manager), `readonly`, `denied` (permissions, a protected folder) or `failed`; `detail` is the system's message. Launchers before 0.2.18 answered a plain-text `409` for all of these |
| `POST` | `/s/<token>/append` | The body added at the END of the file, in place (see *Writing*). Requires `If-Match: <etag>`. `200` `{ etag, size }` (the new tag and length, `ETag` too); `412` + current `ETag` if the file changed on disk; `409` as for `PUT`. Launchers before 0.2.23 answer `405` |
| `POST` | `/s/<token>/save-as?name=<file name>` | "Save as": the system's own save dialog, in front, opened on the session file's folder, offering its type, suggesting `name` (kept to a plain file name, given the file's extension; the file's own name when absent). The body is written where the user picked (see *Writing*), and a NEW session is opened on that file for the same origin. `200` `{ token, name, etag }` (the new session's token, the file's name, never its path); `200` `{ cancelled: true }` when the user cancels; `409` as for `PUT` if the chosen file cannot be written. One dialog at a time: a second request waits for the first dialog to close. Launchers before 0.2.23 answer `405` |
| `POST` | `/s/<token>/copy?suffix=<word>` | The body written BESIDE the file, as `<stem> (<word>)<ext>` (`word`: the page's own for "copy", kept to letters, digits and spaces), and to that same copy again on the next call of the session. `200` `{ name, path }` (the full path, for the page to say where the work went); `409` as for `PUT`. Launchers before 0.2.18 answer `405` |
| `POST` | `/s/<token>/reveal?which=file\|copy` | The file or its copy shown selected in the system's file manager (Explorer `/select`, Finder `open -R`, freedesktop `FileManager1.ShowItems` else the folder). `204`; `404` when there is no copy. Only these two paths: a session names no other |
| `POST` | `/s/<token>/heartbeat` | `204` |
| `DELETE` | `/s/<token>` | `204`, session closed |
| other | | `404` (unknown token) / `405` |

Every response carries `Access-Control-Allow-Origin: <origin>`,
`Access-Control-Expose-Headers: ETag, Content-Range, Accept-Ranges, Content-Length`,
`Vary: Origin`, `Cache-Control: no-store`.

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
- A refused write reads the rest of the request's body before answering
  (tiny_http would read it after, into one buffer the size of what is left:
  gigabytes, for a large file).

### Large files: reading in parts, appending

A project file is a container (ZIP) that may weigh gigabytes (a video project
embeds its media). Reading it whole into the page and writing it whole at
every save does not scale. The container is designed for two things, which
the bridge serves:

- **Reading its parts**: `GET content` with a `Range`, one slice per request,
  streamed from disk.
- **Saving by appending**: the changed entries, then a new central directory,
  written at the END of the file (`POST append`). Nothing already on disk is
  read or written again: appending 1 MiB to a 1 GiB file takes 6 ms where a
  whole `PUT` takes 4.3 s (Windows 11, local SSD).

An append is written **in place**, the one write that is: a temporary file
and a swap would write the gigabytes again. So:

- the file is opened for writing and **locked exclusively** first (Windows:
  no other writer while it is open, and the lock keeps other programs' reads
  out of it for the append's duration; macOS and Linux: an advisory `flock`).
  A lock held by another program is waited for as above, then refused with
  `409` `locked`;
- `If-Match` is checked again once the lock is held, so nothing slips in
  between the check and the write;
- the new bytes are synced (`fsync`, `FlushFileBuffers`) before the answer;
- a failure while writing (a full disk, a page gone mid-request) cuts the
  file back to its length and date, so the page's tag still names it;
- **a crash or a power cut can leave the old content whole followed by a
  torn tail.** This is accepted by design: the container's reader finds the
  last complete central directory and ignores what follows it, so the file
  opens as it was at the last complete save. The bridge always appends at
  the end of what is on disk; a whole write (`PUT`, "Save as") rewrites the
  file without the torn tail.

### Save as

`POST save-as` lets a page offer "Save as" without ever naming a path: the
launcher shows the system's own save dialog (Windows' common item dialog,
macOS' `NSSavePanel`, the XDG desktop portal on Linux, `zenity` where there is
no portal), which asks before replacing a file. The dialog opens in front of
the page: on Windows a background program cannot take the focus by itself,
so the dialog is owned by an empty, always-on-top window of the launcher,
put in front first (its thread shares for a moment the input of the window
in front), and the focus goes back to that window afterwards; macOS raises
the panel above every window. The body is then written the safe way, as for
`PUT` (a new file: written aside and renamed into place), and the page gets
a new session on the new file, for its own origin only. The dialog is
labelled by the system, in its language; the type it offers is named by its
extension (`.<ext>`), so there is no word to translate.

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
  | `bridge` | Kynoko Launcher, any desktop browser | `PUT` with `If-Match`, in place; a large container appends its changes (`append`) and reads its parts (`Range`); "Save as" through the system's dialog (`save-as`) |
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
| Linux | `.deb`, `.rpm`, AppImage | user-level integration in `~/.local` and `~/.config`; the AppImage installs itself on its first start (below) |

The AppImage, downloaded wherever the browser put it, installs itself on its
first start: a copy in `~/.local/share/kynoko-launcher/` (executable) and its
own "Kynoko Launcher" menu entry, so the shortcuts and file types it writes
never point into a Downloads folder someone may tidy. Started from elsewhere,
a newer version replaces the copy, an older one leaves it; "Download and
install" updates the copy. Resetting the choices leaves it installed;
`kynoko-launcher cleanup` removes it with the rest.

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

## 13. Self-update notice, and "Download and install"

No automatic update (Tauri's updater requires its own signing key; to
revisit). The window asks GitHub for the latest release by itself when it
shows (at most once an hour; never while it is hidden, serving files or
assistants: its page cannot tell, the launcher says), and again on "Check
now". All of it at the top of the window: the version beside the name; "Up
to date" (or that it could not be checked) and the catalogue's date beside
"Check now"; under the lead, a newer version's card, its name and two
buttons on one line. No system notification.

- **Download** (with "from GitHub" in small): the release's page, in the
  system's browser.
- **Download and install**, when the release has this system's file:
  - the file is fetched by the launcher, from this repository's releases only
    (`https://github.com/kynoko/kynoko-launcher/releases/download/`), at most
    512 MB, into the user's cache folder (never a roaming profile), and must
    match the SHA-256 GitHub computed when it was uploaded (the API's
    `digest`; the release's `SHA256SUMS.txt` when the API gives none). The
    window only asks for "the latest": no address or path comes from a page,
    and the commands are granted to the settings window alone (capability
    "default"), never to a Kynoko window;
  - **Windows** and the **AppImage** install in place. The launcher closes
    its Kynoko windows first, each through the unsaved-work guard (the page
    asks "Save / Don't save / Cancel"); a window kept open is waited for, up
    to 10 minutes, and the window offers Cancel. When files are open through
    the bridge in a browser, or Kynoko windows are open, the window says first
    what installing closes. Once none is left, it waits for the web engine to
    write what it still holds (FLUSH_GRACE, a countdown in the window), then
    starts the installer in update mode (`/P /UPDATE /R`: a progress bar, no
    question, the version in place not uninstalled, so settings stay, and the
    new launcher started at the end) and quits; the AppImage is replaced by a
    rename and started again once this launcher has quit;
  - **macOS** (the `.dmg`) and a `.deb`/`.rpm` install are handed to the
    system: the disk image opens in the Finder, the package in the software
    centre, and the window says how to finish.
  - An isolated run (end-to-end tests) downloads and checks for real but never
    installs: it writes `update-dry-run.json` instead.

Trust is the same as a download by hand (section 12): the release, its
checksums and its public build. The installer is not signed; started by the
launcher, it carries no download mark, so SmartScreen does not stop it, but
Smart App Control, where enabled, still does.

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
5. ~~Catalogue URL~~ decided: the platform serves it (`GET
   /api/public/launcher-catalogue/`, decision of 2026-09-26).
6. **Linux sandboxed browsers** (Flatpak, Snap): loopback reachable, launching
   with a profile.
7. ~~A local MCP server~~ built (2026-10-11): see section 19. Still open
   there: a tray icon while an assistant is connected, an `.mcpb` bundle for
   Claude Desktop, files named by an assistant (a path, only for the apps'
   file types and in folders the user allowed; the page still receives a
   token, never the path), and sharing the protocol code with Kynoko's remote
   gateway, which serves the assistants that only accept remote servers
   (ChatGPT, Mistral Vibe Work).
8. **The sign-in carried to the browsers** (section 20, proposed): the
   questions it leaves to decide are listed at its end.

## 17. Phases

| Phase | Content |
|---|---|
| 0 - done | Bridge spike: protocol, guards, write back, LNA measured (Edge, Firefox). |
| 1 - done | Skeleton: `files` declaration, file source abstraction with `bridge` and `handle`, generic `/open`, save in place, conflict UI, pre-prompt, no `file_handlers`, desktop install invitation. Office, Photo Studio, Media Studio, then the other apps. |
| 2 - done | Kynoko Launcher v1: catalogue (bundled, and online every 12 h), browsers and profiles, shortcuts, associations, the Kynoko window, agent, inventory and cleanup, self-update, 7 languages. Windows first (2026-09-25, verified end to end: associate Office, double-click a .docx, edit, Ctrl+S on disk, remove everything, registry back to its prior state), then macOS and Linux (v0.2.x). |
| 3 - done, but one item | Packaging on GitHub Actions for the three systems: release on a `v*` tag, stable file names, SHA-256 checksums. The download window lives in every app and says how to open an unsigned build on Windows and macOS. Not said there yet: Windows' Smart App Control, which blocks unsigned programs outright (section 12). |
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
| 2026-10-05 | Large files through the bridge, for the apps' project files (ZIP containers of several GB: a video project embeds its media). `GET content` honours one `Range` (`206`, `416`, `Accept-Ranges`, the slice streamed from disk) and always sends `Content-Length`. `POST append` adds the body at the end of the file IN PLACE, under an exclusive lock, `If-Match` checked again under it, synced before the answer, cut back on a failed write; a crash may leave a torn tail after the old content, which the container's reader skips (it reads the last complete central directory): accepted, since a temporary copy and a swap would write the gigabytes again at every save. `POST save-as` shows the system's save dialog (rfd, MIT: common item dialog, `NSSavePanel`, XDG portal), brought in front of the browser (Windows: an always-on-top owner window put in front first), writes the body where the user picked, the safe way, and answers a new session for the same origin: the page suggests a name, never a path. `meta` lists these `features`. A refused write reads the rest of its body before answering. Older pages are unchanged. |
| 2026-10-05 | A Kynoko app's files look like Kynoko's: their icon is the facade's, cut to Boréal's facet (kynoko-ui's corners: top-left and bottom-right ample, a quarter of the side, the other two sharp, 6 %, as measured on the Kynoko tile), with the Kynoko mark (the platform's own icon, the mushroom tile, read from https://kynoko.com/manifest.webmanifest at run time, kept a week) small in the bottom-right corner the facet frees, parted from the drawing by a transparent ring so that it reads in light and dark folders. Without the mark (never had, offline), the facet alone. A new version writes the associations again, so the icons already set are redrawn on the update. |
| 2026-10-08 | "Download and install" beside "Download" (section 13): asked for by the user, never automatic. The launcher fetches the release's file for its system, checks it against GitHub's SHA-256, and installs it in place on Windows (NSIS update mode, after closing its Kynoko windows through their unsaved-work guard and letting the web engine write) or hands it to the system (macOS disk image, Linux packages); the AppImage is replaced in place. The idle watch does not quit while an install waits. |
| 2026-10-09 | First start, asked for by the user after a first try on Ubuntu and Mint ("no app in the menu, nothing associated"; nothing was, by design, until each app was switched on): every app gets its shortcuts at once, and the window asks once whether the apps open the user's files (all ticked, the types in view, "Associate" or "Later"), rather than taking the types over silently: on Linux and macOS that answer makes them the default. A launcher already set up by hand is left as it is. The AppImage installs itself (a copy in `~/.local/share/kynoko-launcher`, its own menu entry), which also gives it a place in the menu; the `.deb` and `.rpm` have theirs already. |
| 2026-10-09 | Asked for by the user: what is recommended is also the default for opening. An app met for the first time whose followed browser it does not recommend gets the first installed one it does (section 5): Office the Kynoko window, Media Studio, Photo Studio, PDF Editor and Games a Chromium browser when the system's is Firefox, QR Code Studio left on Firefox (it recommends both). Linux now knows the system's browser (`xdg-settings`, `xdg-mime`), which the window also shows. |
| 2026-10-11 | The specification says what was built: the agent never had the tray / menu-bar icon section 3 planned, so a bridge session ends with its page or after 10 min without heartbeat, never from a tray. Such an icon comes with a resident mode, when a feature needs the agent to stay up. Status line and phases brought up to date. |
| 2026-10-11 | AI assistants installed on the computer use the Kynoko apps over MCP (section 19): `kynoko-launcher mcp` is a pipe between the assistant's standard input and output and the agent, which it starts when none runs; the agent answers the protocol (dual era, 2026-07-28 and the handshake versions before it) with the tools each app declares in its manifest, and relays each call to a window of the app it opens for the assistant, in the app's browser, at a `#kynokoAutomation=` fragment; the page asks its user first, runs the call in its own engine and history, and its Stop keeps the assistant out of the app for 10 minutes. One loopback WebSocket listener for pages and assistants alike, guarded like the bridge (exact Host, exact Origin for a page, a token in the state folder and no Origin for an assistant). The window's "AI assistants" switches add Kynoko to the configuration of Claude Desktop, LM Studio, Cursor, Qwen Code, VS Code and Codex (only our entry, the rest kept, a file with comments left alone, a copy kept aside), inventoried, so removed on reset and uninstall. No tray icon yet; one loopback WebSocket rather than a Tauri channel for the Kynoko window too. |
| 2026-10-12 | Seen with Claude Desktop on Layout Studio: two calls sent together, before the user had answered, opened two windows, each asking the user. A call now finds the window opened for the app a moment ago and waits for it; `kynoko_open` finds the window already open instead of always opening one. The apps' "AI assistants" entry opens the window on that section: `kynoko-launcher://settings?app=<code>&section=assistants` (skeleton v0.111.0, `kynokoOpenLauncher(code, 'assistants')`); a link names only a section the window knows, and an older launcher shows the app. |
| 2026-10-12 | Asked for by the user: the update moves to the top of the window, condensed (the version beside the name, "Up to date" and the catalogue's date beside "Check now", a newer version's card of one line under the lead), and the window checks by itself when it shows, at most once an hour, never while hidden (section 13). Still no automatic install. |
| 2026-10-13 | Proposed, asked for by the user, not built: the launcher signs in to Kynoko once (OAuth with PKCE, redirect to 127.0.0.1, a rotating device token in the system's credential store) and hands the sign-in to every browser it opens an app in, through a one-time code of 60 seconds that the platform's API exchanges for an ordinary browser session before sending the browser on to the app (section 20). Decided the same day: the launcher signs in in a Kynoko window, device token 90 days sliding (one year at most), windows opened for an assistant signed in too, Android later. Without it, each browser (the Kynoko window, Firefox, Chrome, each profile) needs its own sign-in. Platform side: `backend/specifications/LAUNCHER_SIGN_IN.md` of the platform repository. |
| 2026-09-25 | Product renamed **Kynoko Launcher** (was "Kynoko Applications", too easily confused with the apps themselves); repository `kynoko/kynoko-launcher`, binary and packages `kynoko-launcher`. |

## 19. Assistants (MCP): normative protocol

An AI assistant installed on the computer (Claude Desktop, Codex, Qwen Code,
Mistral Vibe Code, LM Studio, Jan, VS Code, Cursor) uses the Kynoko apps
through the Model Context Protocol: it creates and changes documents with
the app's own engine, in a window the user sees. The apps' side is the
skeleton's (`provideKynokoAutomation`); this section is the launcher's.

### Processes

- `kynoko-launcher mcp` is what an assistant starts. It runs before
  anything else, without Tauri (no window, no single-instance hand-over), and
  is a pipe: each line on its standard input goes, as it is, to the agent
  over the loopback, each answer comes back as a line on its standard
  output. What goes wrong is said on standard error.
- Where the agent listens is in the state folder, `automation.json`
  (`{ "port", "token" }`), rewritten at each start of the relay, readable by
  the user only (0600 on Unix; the state folder is the user's own on
  Windows). No answer there: the pipe starts `kynoko-launcher agent`, apart
  from itself (detached, out of the assistant's job when it allows it), and
  waits up to 25 s for it. A running launcher gets `agent` through its
  single-instance hand-over and starts its relay.
- The agent stays up while an assistant is connected or a page works for
  one, on top of the idle rules of section 3.

### The listener

One WebSocket listener on `127.0.0.1`, a free port, for assistants and pages
alike. Every handshake must carry `Host: 127.0.0.1:<port>` exactly (421
otherwise: DNS rebinding).

- `/c/<token>`: an assistant. The token of `automation.json`, compared in
  constant time, and NO `Origin` header (403 otherwise): a web page always
  sends one, so no page can pose as an assistant.
- `/a/<token>`: a page. 256 random bits minted for ONE page of ONE app,
  good only for the origin of that app's address (exact `Origin`, 403
  otherwise; unknown token: 404). Forgotten 5 minutes after it was made if
  the page never came, 10 minutes after it left.

### MCP

Dual era (MCP 2026-07-28, "Versioning and Compatibility"): `initialize`
selects a handshake version (2024-11-05 to 2025-11-25; an unknown one gets
the latest of them); a request carrying `_meta`
`io.modelcontextprotocol/protocolVersion` is served statelessly
(`server/discover`, `resultType`, `ttlMs` and `cacheScope` on lists), and an
unsupported modern version gets `-32022` with the supported list. Tools only.
The assistant's name (`clientInfo`) is shown by the pages.

The tools:

- `kynoko_apps` (read): the apps an assistant can use, their tools, whether
  a window is open for it; `kynoko_open` (`app`, `facade`): the window open
  for it, else a new one.
- Each app's own: the `automation` block of the app's manifest,
  `<app address>/assets/kynoko-app.json` (written at build time from the
  app's `core/automation.json`): `{ v: 1, prefix, tools: [{ name, title?,
  description, input, effect, facade?, facadeFrom? }] }`; `facadeFrom` names
  the argument that chooses the facade (Layout Studio's product). Read from every live app of the
  catalogue at once, kept 5 minutes, the last good copy of each app kept in
  `automation-tools.json` for when it cannot be reached. Checked: a prefix of
  2 to 16 lowercase letters or digits, never `kynoko`, unique (first app
  wins); names `<prefix>_...`, unique; an object schema; `effect` among
  `read`, `write`, `destructive`, which become `readOnlyHint` and
  `destructiveHint`. A result passes text and pictures only.

A call to an app's tool goes to the window of that app that connected last;
none: one is opened (below) and the call waits up to 2 minutes for the user
to allow the assistant there, then up to 3 minutes for the answer. A tool
call runs on its own thread: the assistant's other messages go on. One
window per app: while a window opened for an assistant waits for its user's
answer (those 2 minutes), every call to that app, from any assistant, waits
for that same window instead of opening another, which would ask the user
again for the same work.

### The page

The window is opened like a file's (section 8): in the app's browser, at
`<app address>/<facade>#kynokoAutomation=127.0.0.1:<port>/<token>&kynokoLauncher=1`
(an ordinary Firefox window until that profile was seen reaching the
launcher). The page asks its user whether the assistant may work there, then
connects to `/a/<token>`. Messages, one JSON object each, protocol 1:

| From | Message | Meaning |
|---|---|---|
| launcher | `{ type: "welcome", protocol, client? }` | at connection; `client`: the assistant's name |
| launcher | `{ type: "client", client? }` | the assistant connected or left |
| launcher | `{ type: "call", id, tool, args }` | run a tool |
| page | `{ type: "hello", protocol, app, tools }` | who it is: another app or protocol is let go (4403, 4426) |
| page | `{ type: "result", id, result }` | MCP's own result shape (`content`, `structuredContent?`, `isError?`) |
| page | `{ type: "error", id, code, message }` | why it could not; said to the assistant |
| page | `{ type: "bye", reason: "stopped" }` | the user pressed Stop: calls to that app are refused for 10 minutes |

Only the page a call was sent to may answer it. A page that leaves fails its
calls at once.

### The assistants' configuration

The window's "AI assistants" section shows the assistants the launcher
recognises on the computer, one switch each, which adds or removes ONE
entry, `kynoko` (`{ "command": "<launcher>", "args": ["mcp"] }`):

| Assistant | File |
|---|---|
| Claude Desktop | `<config dir>/Claude/claude_desktop_config.json`, `mcpServers` |
| Claude Desktop (Microsoft Store) | `%LOCALAPPDATA%/Packages/Claude_*/LocalCache/Roaming/Claude/claude_desktop_config.json` (a packaged app's Roaming folder), `mcpServers` |
| LM Studio | `~/.lmstudio/mcp.json`, `mcpServers` |
| Cursor | `~/.cursor/mcp.json`, `mcpServers` |
| Qwen Code | `~/.qwen/settings.json`, `mcpServers` |
| VS Code | `<config dir>/Code/User/mcp.json`, `servers` (`type: "stdio"`) |
| Codex | `~/.codex/config.toml`, `[mcp_servers.kynoko]` |

The rest of the file is kept as it is, key order included; a file that is
not plain JSON (comments) is left alone and the window says to add Kynoko by
hand, with what to add (also offered for any other assistant). Before the
first change, the file as it was is copied beside it
(`<file>.kynoko-backup`). Each entry is in the inventory
(`assistantEntry`): "Reset everything" and the uninstaller take it out. An
isolated run (`KYNOKO_LAUNCHER_HOME`) works in `<home>/home` and never in the
user's files.

## 20. The Kynoko account: the sign-in carried to the browsers

Status: **proposed 2026-10-13, decided the same day (see "Decisions"),
not built.** The platform side (authorization
server, device tokens, codes, the exchange, revocation) is specified in the
platform repository, `backend/specifications/LAUNCHER_SIGN_IN.md`; this
section is the launcher's part.

### Why

The Kynoko session is a cookie (`shared_domain_jwt`, on `.kynoko.com`), and a
cookie belongs to one browser. The launcher opens each app in the browser
chosen for it: the Kynoko window for one, Firefox or Chrome (and a profile)
for another. Without help, the user signs in once per browser, and again
whenever a session expires in one of them.

### What the launcher does

- **Signing in, once per computer.** The window's "Kynoko account" section
  offers "Sign in". The launcher opens a KYNOKO WINDOW (decided 2026-10-13;
  its own engine, the one its apps open in) at
  `https://kynoko.com/oauth/authorize` (public client `kynoko-launcher`, PKCE
  S256, `state`, `redirect_uri` `https://kynoko.com/oauth/launcher-callback`).
  The user signs in there with Kynoko's own page (TOTP, passkeys) and allows
  "Kynoko Launcher on <computer name>"; the Kynoko windows are signed in at
  the same stroke, since the page runs in their profile. The window's
  navigation handler catches the redirect to the callback address (it never
  loads it), checks `state`, closes the window, and the launcher exchanges the
  code and its `code_verifier` for an access token (10 minutes) and a device
  token (rotating refresh token). RFC 8252 prefers the system's browser for
  fear of a THIRD-PARTY app reading the credentials typed into it; the
  launcher is Kynoko's own, and the window it signs in is where the user
  works most. What it gives up: the browser's password manager (passkeys
  work in WebView2 and WKWebView; WebKitGTK to check).
- **Keeping it.** The device token goes to the system's credential store, and
  nowhere else: Windows Credential Manager (DPAPI), the macOS Keychain, the
  Secret Service on Linux. Without a store (a Linux session without one), the
  launcher says that it cannot remember the sign-in and keeps it for the run
  only; it never writes it to a file. The access token lives in memory.
- **Handing it over.** Each time the launcher opens an app page (`launch`, a
  file, an assistant's window; in any browser, the Kynoko window included),
  it first asks the platform for a sign-in code for that address (`POST
  /api/launcher/sign-in-code`, `{ "to": <the address without its fragment> }`)
  and opens the address the platform answers instead,
  `https://api.kynoko.com/launcher/sign-in?code=...&to=...`, with the app
  address's fragment (`#kynokoBridge=`, `#kynokoAutomation=`,
  `kynokoLauncher=1`) appended to it: the API sets the browser's own session
  and sends it on (303) to the app, and the browser carries the fragment
  through the redirect (a `Location` without one inherits the request's, RFC
  9110 section 10.2.2), so the platform never sees it. The code is one-time,
  valid 60 seconds, bound to the app's origin and to the public IP address of
  the request. It costs the opening one request to the platform before, and
  one redirect after. The Firefox
  gateway (section 5) is handed that address like any other: it navigates to
  any `https://*.kynoko.com/` address.
- **Never in the way.** No code when the user is not signed in to the
  launcher, when the address is not an app of the catalogue (an `appUrls`
  override to another host, except the platform's dev host), or when the
  platform does not answer within 2 seconds: the app opens as today, and
  offers its own sign-in. The launcher waits for nothing else.
- **Rotation.** Each use of the device token replaces it, written back to the
  store before the new access token is used. A device token refused as
  already used (the platform's theft detection) or revoked signs the
  launcher out, quietly: the "Kynoko account" section says so and offers
  "Sign in" again.
- **Signing out.** "Sign out" revokes the device token (`POST /oauth/revoke`)
  and removes it from the store. The browsers keep their sessions (ordinary
  ones), unless the user ticks "and in the browsers it signed me in to".
  "Reset everything" and the uninstaller sign out too (inventory entry
  `launcherSignIn`).

### What it never does

- It exposes no local endpoint that hands out a code or a token: a web page
  can ask it for nothing. It asks for a code only to open, itself, an app of
  the catalogue.
- It never sees the browser's cookie or the session JWT: only its device
  token, its access token and one-time codes.
- It never replaces a browser's session by another account's: the platform
  leaves a browser signed in as someone else untouched.
- It logs no token and no code.

### The window

A "Kynoko account" line at the top, under the update line (section 13):
signed out, "Sign in to open your apps signed in, in every browser" and a
"Sign in" button; signed in, the account's name and e-mail address, "Sign
out", and a line on what it does ("Your apps open signed in, in every browser
Kynoko Launcher opens them in"). While the sign-in window is open: "Finish
signing in in the window that opened" and Cancel (which closes it).

### Isolated runs and dev

An isolated run (`KYNOKO_LAUNCHER_HOME`) never reads or writes the system's
store: it keeps the device token in memory only. The platform's address is
`https://api.kynoko.com/` and `https://kynoko.com/`, overridable in settings
(`accountApiUrl`, `accountUrl`) for the dev platform.

### Decisions (2026-10-13)

1. The launcher signs in in a Kynoko window, not the default browser.
2. Device token: 90 days sliding, one year at most; code: 60 seconds.
3. The same-IP check between a code's request and its exchange: strict (IPv4
   equal, IPv6 in the same /64), its refusals counted by the platform, eased
   if they prove a nuisance (proposed, awaiting the user's agreement).
4. Windows opened for an assistant are signed in too.
5. Android: the same later, with the Keystore.
