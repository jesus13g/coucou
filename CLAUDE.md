# Coucou — guide for AI coding agents

Coucou is a desktop app for **Windows and Linux**: Mochi, a small animated character living at the top centre of the screen, shows Claude Code sessions and a few integrations, and lets the user approve, answer, chat and drop files from the island. It is not a macOS app and must not be built or shipped for macOS.

## Where things are
- `windows/` — the app (Tauri 2), for both Windows and Linux despite the folder name.
  - `windows/src/` — front end (TypeScript, no framework): island, Mochi (Canvas 2D), views, settings window.
  - `windows/src-tauri/` — Rust backend: window, relay server, Claude API, pollers, key store.
  - `windows/hook/` — `coucou-hook`, the Claude Code relay.
  - `windows/sounds/` — the 28 WAV sounds.
  - `windows/linux/` — Arch `PKGBUILD` and desktop entry. `windows/LINUX.md` — Linux guide.
- `docs/SPEC.md`, `docs/INTEGRATIONS.md` — behaviour, views, states, integrations (in French).
- `design/prototype/coucou.html` — original prototype, the visual source of truth. `design/captures/` — target screenshots.
- `docs/*.html` — the GitHub Pages site (privacy, terms, support, legal notice).

## Build
```
cd windows && npm install && npm run tauri dev    # development
cd windows && npm run pack                        # installer (Windows) or .deb/.rpm/.AppImage (Linux)
cd windows && cargo test                          # Rust tests (build the hook first: cargo build --release -p coucou-hook)
```

## Rules
- Tauri 2: Rust backend + TypeScript front end. Keep dependencies minimal. The character is drawn in code (Canvas 2D), no Rive/Lottie/images.
- Platform code lives behind `#[cfg(windows)]` / `#[cfg(target_os = "linux")]`; shared code stays shared. Every change must keep both Windows and Linux building.
- Secrets live in the OS key store (Windows Credential Manager, Linux Secret Service), never on disk or in git.
- No telemetry. Network calls only to services the user configured.
- Never block Claude Code: if the app doesn't answer, the hook exits immediately.
- Never overwrite `~/.claude/settings.json`: dated backup, merge, show the diff, write only after the user confirms.
- Never send an email or approve a Claude Code permission without an explicit click.
- Performance: 0 % CPU when the island is hidden.
- Keep the app identifier `fr.louisraille.coucou` (key-store entries and settings depend on it).
- Visual changes must match the prototype and the screenshots in `design/captures/`.
