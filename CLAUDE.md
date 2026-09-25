# Kynoko Applications - working rules

**This repository is PUBLIC** (GitHub `kynoko/kynoko-applications`), unlike
its neighbours in `apps/kynoko/` and everything in `apps/kynoko-apps/`, which
are private (GitLab). Everything committed here is published to the world.

Never commit:

- secrets of any kind: tokens, keys, passwords, `.ini` credentials, cookies;
- internal or dev URLs hard-coded in the code (dev domains, IPs, ports of the
  dev machine). They belong to configuration that the user can override;
- dependencies on private registries or packages: `@common/*` (kynoko-ui,
  skeleton), php-lib, anything that needs credentials to install. The build
  must work on a fresh GitHub Actions runner with no secrets;
- brand assets (Kynoko logo, app icons): they are trademarks, not Apache-2.0
  (see TRADEMARKS.md), and are fetched from the catalogue at run time;
- content copied from the private repositories (code, specs, internal docs)
  without checking that it is meant to be public.

Rules:

- Everything added is Apache-2.0. Third-party material keeps its license and
  is listed (fonts: SIL OFL; no GPL code in the shipped binary).
- Code, comments and docs in English (public audience). User-facing strings go
  through i18n, in the 7 Kynoko languages, RTL included.
- The design is in `docs/SPEC.md`; update its decision log when a decision
  changes.
- Tests must kill every process they start (browsers, bridges, servers).
