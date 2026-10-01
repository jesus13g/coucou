# Changelog

## Unreleased

- Linux support: the Tauri app in `windows/` now also builds for Linux (WebKitGTK, X11/XWayland), with Claude Code hooks over a private Unix socket, keys in the Secret Service, and a PKGBUILD for Arch — see `windows/LINUX.md`
- Compact island on screens without a notch (#22) — thanks @Kamasoutra
- Only web links (http/https) open from the notch; other kinds of links from Claude or integrations are ignored (#16) — thanks @Cris1670
- Hook socket limited to your own user account, with size and time limits; logs no longer keep commands, n8n data or full URLs, and stay under 1 MB (#16) — thanks @Cris1670 and @Vignesh-Thangamariappan
