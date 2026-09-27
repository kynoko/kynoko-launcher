# Contributing to Kynoko Launcher

Thank you for helping. This repository is **public**, unlike the rest of
Kynoko, which is private: everything committed here is published to the
world. These rules keep it that way safely.

## Never commit

- secrets of any kind: tokens, keys, passwords, `.ini` credentials, cookies;
- internal or development addresses hard-coded in the code (dev domains, IPs,
  ports of a development machine). They belong to configuration the user can
  override;
- dependencies on private registries or packages: `@common/*` (kynoko-ui,
  skeleton), php-lib, anything that needs credentials to install. The build
  must work on a fresh GitHub Actions runner with no secrets;
- brand assets (Kynoko logo, app icons): they are trademarks, not Apache-2.0
  (see [TRADEMARKS.md](TRADEMARKS.md)), and are fetched from the catalogue at
  run time;
- content copied from Kynoko's private repositories (code, specifications,
  internal documents) without checking that it is meant to be public.

## Rules

- Everything added is Apache-2.0. Third-party material keeps its license and
  is listed (fonts: SIL OFL; no GPL code in the shipped binary).
- Code, comments and documentation in English. User-facing strings go through
  i18n, in the 7 Kynoko languages, right-to-left included.
- The design is in [docs/SPEC.md](docs/SPEC.md); update its decision log when
  a decision changes.
- Tests must stop every process they start (browsers, bridges, servers). An
  end-to-end test on a machine where Kynoko Launcher is installed runs it
  isolated (`KYNOKO_LAUNCHER_HOME`, see `app/src-tauri/src/settings.rs`).
