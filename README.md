# Kynoko Launcher

The desktop companion of the [Kynoko](https://kynoko.com) web apps, for
Windows, macOS and Linux.

- Installs the Kynoko apps, with shortcuts to each app and its tools.
- Opens your files in the right app with a double-click, in the browser you
  choose (any installed browser, per app if you like).
- Lets the app save back to the original file (a large project by adding
  only what changed to its end), or under a new name you pick in your
  system's own "Save as" dialog.
- Removes everything it created when you uninstall it, and restores your
  previous default apps.

Status: **released for Windows, macOS and Linux** (v0.2.x, see
[Releases](https://github.com/kynoko/kynoko-launcher/releases)). Windows has
been used the most; macOS is built and packaged by the CI, and has been tried
less than the other two. Design: [docs/SPEC.md](docs/SPEC.md).

## Build

Requirements: Rust (stable), Node.js 22+, and on Windows the MSVC build
tools and WebView2 (present on Windows 10/11).

```
cd app
npm install
npx tauri build        # app/src-tauri/target/release/bundle/nsis/*-setup.exe
npx tauri dev          # run from source
```

A release bundles the platform's current catalogue
(`.github/workflows/release.yml`), and falls back to the committed snapshot
`app/src-tauri/catalogue.json` when the platform cannot be reached.
`tools/build-catalogue.mjs` builds such a snapshot from the apps' own
`kynoko-app.json`.

## Unsigned builds

Releases are not signed with a paid identity. Your system will warn you the
first time; the download window of every Kynoko app gives the steps for
Windows and macOS. Every release publishes SHA-256 checksums, and is built by
the public GitHub Actions workflows of this repository.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md): this repository is public, and a few
rules keep it safe.

## License

[Apache License 2.0](LICENSE). The Kynoko name and logos are not covered by
the license: see [TRADEMARKS.md](TRADEMARKS.md).
