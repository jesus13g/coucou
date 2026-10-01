# Contributing to Coucou

Thanks for wanting to help Mochi grow up! 🫶

Coucou runs on **Windows and Linux**. The app lives in `windows/` (Tauri 2: Rust + TypeScript) and the same sources build on both systems.

## Getting started

Requirements: [Rust](https://rustup.rs), Node 20+, and the platform's webview toolchain — the MSVC build tools on Windows, WebKitGTK 4.1 and libayatana-appindicator on Linux (see [`windows/LINUX.md`](windows/LINUX.md)).

```bash
cd windows
npm install
npm run tauri dev      # live-reloading development build
```

Checks to run before a pull request:

```bash
cd windows
npx tsc --noEmit
cargo build --release -p coucou-hook && cargo test
```

## Good first contributions

- A new integration (a poller in `src-tauri/src/integrations.rs` + a pill + a detail card).
- A new emote or sound for Mochi.
- Bug fixes — please describe how to reproduce.

## Rules of the house

- Platform-specific code goes behind `cfg` gates; anything that can be shared is shared. Both Windows and Linux must keep building.
- Keep dependencies to what is really needed.
- Secrets go in the OS key store (Windows Credential Manager, Linux Secret Service), never on disk or in git.
- No telemetry, no network calls except to services the user configured.
- Never block Claude Code: if the app doesn't answer, the hook must exit right away.
- Never write `~/.claude/settings.json` without a backup and the user's confirmation.
- Keep it light: 0 % CPU when the island is hidden.

## Pull requests

- One topic per PR, with a short GIF or screenshot for anything visual.
- Build must pass with no new warnings.
