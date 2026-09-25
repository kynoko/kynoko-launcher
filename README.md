# Kynoko Launcher

The desktop companion of the [Kynoko](https://kynoko.com) web apps, for
Windows, macOS and Linux.

- Installs the Kynoko apps, with shortcuts to each app and its tools.
- Opens your files in the right app with a double-click, in the browser you
  choose (any installed browser, per app if you like).
- Lets the app save back to the original file.
- Removes everything it created when you uninstall it, and restores your
  previous default apps.

Status: **design phase**. See [docs/SPEC.md](docs/SPEC.md). The loopback
bridge prototype lives in [spikes/loopback-bridge](spikes/loopback-bridge/).

## Unsigned builds

Releases are not signed with a paid identity. Your system will warn you the
first time; the steps to proceed will be documented for each system. Every
release publishes SHA-256 checksums, and is built by the public GitHub Actions
workflows of this repository.

## License

[Apache License 2.0](LICENSE). The Kynoko name and logos are not covered by
the license: see [TRADEMARKS.md](TRADEMARKS.md).
