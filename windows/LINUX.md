# Coucou for Linux

The Windows app in this folder is a Tauri app, and the same sources build a
native Linux app: Mochi lives at the top centre of your screen, exactly like on
Windows. Developed and tested for **Arch Linux**; any distribution with
WebKitGTK 4.1 works.

What works: the island and every view, Mochi and the greeting, the 28 sounds,
Claude Code hooks with **Allow / Deny from the island**, the chat, dropping
files, the integration pills, the tray menu, launch at login.

## Install on Arch

The `PKGBUILD` in [`linux/`](linux/PKGBUILD) builds from git like an AUR
`-git` package:

```sh
cd windows/linux
makepkg -si
```

It clones `https://github.com/jesus13g/coucou` (default branch). To package the
commit you have checked out instead:

```sh
cd windows/linux
COUCOU_REPO="git+file://$(git rev-parse --show-toplevel)#branch=$(git branch --show-current)" makepkg -si
```

It installs `/usr/bin/coucou`, the relay in `/usr/lib/Coucou/coucou-hook`, a
desktop entry and the icons. Then start **Coucou** from your launcher, or run
`coucou`.

### Runtime dependencies

Pulled in by the package; listed here for a manual build:

```sh
sudo pacman -S --needed webkit2gtk-4.1 gtk3 libayatana-appindicator libx11 \
  gst-plugins-base gst-plugins-good xdg-utils
```

- **A Secret Service provider** for API keys: `gnome-keyring` (GNOME, most
  setups), KWallet (Plasma, already there) or KeePassXC with Secret Service
  enabled. Without one, saving a key fails with an error — keys are never
  written to disk as a fallback.
- **A compositor** for the transparent island. GNOME, Plasma, Hyprland and sway
  have one; on i3/Openbox add `picom`.
- **`xorg-xwayland`** on a Wayland session (see below).
- `gst-plugins-good` is what plays Mochi's sounds inside WebKitGTK.

## Build it yourself

```sh
sudo pacman -S --needed base-devel rust nodejs npm webkit2gtk-4.1 \
  libayatana-appindicator librsvg
cd windows
npm install
npm run tauri dev      # live-reloading development build
npm run pack           # .deb / .rpm / .AppImage in windows/release/
```

`target/release/coucou` also runs on its own after `npx tauri build --no-bundle`.

## Claude Code

**Tray icon → Settings… → Claude Code → Install hooks…** shows the exact diff of
what will change in `~/.claude/settings.json`, the dated backup it will take,
and writes nothing until you click. Same rules as on macOS and Windows: your own
hooks are never touched, uninstall removes only Coucou's entries.

The relay, `coucou-hook`, is copied to `~/.local/share/coucou/bin/` at launch
and talks to the app over a Unix socket in `/run/user/<uid>/coucou/` — a
directory only your user can enter, and both ends check the other's uid with
`SO_PEERCRED`. If Coucou is closed the relay exits in a few milliseconds; if
nobody answers a permission request, Claude Code asks in the terminal as usual.
**Claude Code is never blocked.** It works from any terminal: Konsole, GNOME
Terminal, kitty, Alacritty, VS Code…

"Open terminal" opens the working folder in `code`, `code-oss`, `codium` or
`cursor` — the first one on your `PATH` — or in your file manager otherwise.

## Wayland

Wayland deliberately does not let an app place its own window, keep it above
others, or read the cursor outside of it — and the island needs all three. So
on a Wayland session Coucou runs through **XWayland** automatically (it sets
`GDK_BACKEND=x11` for itself). GNOME and Plasma should need nothing more.

- **Hyprland** — XWayland windows don't float by default. Add rules for the
  island (its title is exactly `Coucou`; the settings window is
  `Settings — Coucou`):

  ```ini
  windowrulev2 = float, class:^(coucou)$, title:^(Coucou)$
  windowrulev2 = pin, class:^(coucou)$, title:^(Coucou)$
  windowrulev2 = noborder, class:^(coucou)$, title:^(Coucou)$
  windowrulev2 = noshadow, class:^(coucou)$, title:^(Coucou)$
  windowrulev2 = noblur, class:^(coucou)$, title:^(Coucou)$
  windowrulev2 = noanim, class:^(coucou)$, title:^(Coucou)$
  ```

  (Hyprland renames rule keywords between releases; adapt to yours.)
- **sway / i3** — the island is a utility window, so it floats on its own. To
  keep it on every workspace without a border:

  ```
  for_window [class="coucou" title="^Coucou$"] sticky enable, border none
  ```

`COUCOU_NATIVE_WAYLAND=1 coucou` skips the XWayland switch if you want to
experiment; expect the island to be placed and stacked however the compositor
decides.

## Tray icon

The tray menu (Open, Settings…, Pause, Quit) uses AppIndicator / StatusNotifier.
Plasma, Hyprland/sway bars (waybar `tray` module) and most panels show it. On
**GNOME** install the *AppIndicator and KStatusNotifierItem Support* extension
(`gnome-shell-extension-appindicator`), or you'll have no way to open Settings.

## Where things live

| What | Where |
|---|---|
| Preferences | `~/.config/coucou/settings.json` (`$XDG_CONFIG_HOME`) |
| Log | `~/.local/share/coucou/coucou.log` |
| Relay | `~/.local/share/coucou/bin/coucou-hook` |
| Dropped files (swept after 7 days) | `~/.local/share/coucou/inbox/` |
| Relay socket | `/run/user/<uid>/coucou/hook.sock` |
| API keys | Secret Service, service `fr.louisraille.coucou` |
| Launch at login | `~/.config/autostart/` (toggled from Settings) |

## Troubleshooting

- **The island is a black rectangle** — no compositor is running (i3, Openbox…):
  start `picom`.
- **Blank or flickering window** — Coucou already sets
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` (the usual fix on NVIDIA). If it persists,
  try `WEBKIT_DISABLE_COMPOSITING_MODE=1 coucou`.
- **No sound** — install `gst-plugins-good`.
- **"Can't save the key"** — no Secret Service is running: install and unlock
  `gnome-keyring`, or enable it in KWallet / KeePassXC.
- **On GNOME the island sits under the top bar** — Mutter may keep windows out
  of its panel. It still works; hover just below the bar to wake Mochi.
- Anything else: `~/.local/share/coucou/coucou.log`.

## What's different from the Mac version

The same list as Windows (no notch, approval from any terminal, no email
sending, no window-attach), plus: the cursor is read through X11, so on Wayland
the island only reacts once the pointer is over it or over another XWayland
window — which is all hover and click need.
