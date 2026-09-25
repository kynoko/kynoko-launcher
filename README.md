# Kynoko Launcher

The desktop companion of the [Kynoko](https://kynoko.com) web apps, for
Windows, macOS and Linux.

- Installs the Kynoko apps, with shortcuts to each app and its tools.
- Opens your files in the right app with a double-click, in the browser you
  choose (any installed browser, per app if you like).
- Lets the app save back to the original file.
- Removes everything it created when you uninstall it, and restores your
  previous default apps.

Status: **first working build, Windows**. It lists the apps and every
installed browser, associates an app's file types (and removes them all
again), opens a double-clicked file in the right app through the loopback
bridge, and lets the app save it back in place. macOS, Linux, shortcuts and
the online catalogue come next. Design: [docs/SPEC.md](docs/SPEC.md).

## Build

Requirements: Rust (stable), Node.js 22+, and on Windows the MSVC build
tools and WebView2 (present on Windows 10/11).

```
cd app
npm install
npx tauri build        # app/src-tauri/target/release/bundle/nsis/*-setup.exe
npx tauri dev          # run from source
```

`tools/build-catalogue.mjs` regenerates the catalogue bundled in the build
(`app/src-tauri/catalogue.json`) from the apps' own `kynoko-app.json`.

## Unsigned builds

Releases are not signed with a paid identity. Your system will warn you the
first time; the steps to proceed will be documented for each system. Every
release publishes SHA-256 checksums, and is built by the public GitHub Actions
workflows of this repository.

## License

[Apache License 2.0](LICENSE). The Kynoko name and logos are not covered by
the license: see [TRADEMARKS.md](TRADEMARKS.md).
