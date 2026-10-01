# Changelog

## Unreleased

- Coucou is now a Windows and Linux app only: the macOS app, its build, tests, scripts and workflows are gone; the sounds moved to `windows/sounds/`
- Linux support: the Tauri app in `windows/` now also builds for Linux (WebKitGTK, X11/XWayland), with Claude Code hooks over a private Unix socket, keys in the Secret Service, and a PKGBUILD for Arch — see `windows/LINUX.md`
