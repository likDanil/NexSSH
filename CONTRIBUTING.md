# Contributing to NexSSH

Thanks for helping! A few principles keep the project small and fast:

* **The terminal comes first.** Before adding UI, ask: does the user need to see this all
  the time while working in the terminal? If not, it belongs in the command palette, a
  context menu or a dialog.
* **Mind the weight.** Avoid new dependencies for things that are easy to write; check
  startup time, memory and bundle size when adding features.
* **SSH logic lives in `core`.** The `desktop` crate only wires IPC; the UI never
  implements protocol behaviour.
* **Secrets never touch our files.** Use `core::secrets` (the OS keychain).

## Setup

```sh
npm install
npm run dev
```

See the README for platform prerequisites and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
for an overview.

## Before opening a pull request

```sh
npm run check
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Integration tests against a local OpenSSH server: see *Development* in the README.

* Keep changes focused; describe what and why in the PR.
* UI changes: include a screenshot, check the Light and one dark theme, and both languages
  (long Russian labels are a good test for layouts).
* New IPC commands: update `ui/src/lib/api.ts` and `ui/src/lib/types.ts` together with the Rust side.

## Texts and translations

Every text people see comes from a catalog — never write it inline:

* **Interface:** `ui/src/lib/locales/en.ts` is the reference; add the same key to
  `ru.ts` (TypeScript fails the build if a key is missing). Use `t('key', { name })` in code,
  `tn()` for numbers (plural forms), and `<Rich>` when a placeholder needs markup.
* **Backend** (connection progress, errors): `core/src/i18n.rs`, one entry per message with
  every language. Messages are lowercase fragments so they can be nested; the UI capitalizes them.

**Adding a language:** copy `ui/src/lib/locales/ru.ts`, translate it, register it in
`CATALOGS` and `LANGUAGES` (`ui/src/lib/i18n.svelte.ts`), then add a variant to `Lang` and a
line to every message in `core/src/i18n.rs` — the compiler lists what is missing.
