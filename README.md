# Frost Widgets

Tiny macOS-style glass desktop widgets for Windows 10/11. Native Rust + Direct2D, no browser engine.

Widgets: clock, weather, now playing (any app or browser), system rings, calendar, battery, reminders, network, analog clock, photos, messages, plus a desktop pet that lives on the taskbar.

## Install

Run `dist\FrostWidgets-Setup-<version>.exe` (under 1 MB).

- Installs just for the current user to `%LOCALAPPDATA%\Programs\Frost Widgets`, so no admin prompt.
- Adds a Start menu shortcut, an optional desktop shortcut and an optional startup entry.
- Appears in Settings › Apps › Installed apps, where you can uninstall it.
- Silent install: `FrostWidgets-Setup-<version>.exe --silent`
- Silent uninstall: `uninstall.exe --uninstall --silent`

Uninstalling keeps your settings and reminders unless you tick "Also delete my settings and reminders".

## Use

- **Settings:** right-click any widget or the tray icon, then choose Settings. Tabs: General (start with Windows, lock positions, reset layout), Appearance (theme, accent color, glass strength, widget size), Widgets (turn each on or off), Clock & weather, About.
- **Reminders:** press + on the Reminders widget.
- **Desktop pet:** a small blue-and-purple pixel critter in a knit scarf lives on top of the taskbar. It walks, sits, types on its laptop and naps. Click it to make it hop, drag it to carry it (it drops back onto the taskbar), and right-click it to change its behavior (Do everything, Stay calm, Keep walking, Work on laptop, Nap), walking speed and size, or to hide it. The same options are under Settings › Pet and in the tray menu. It hides while an app is full screen, and when you're away for 10 minutes it falls asleep.
- **Move a widget:** drag it. It snaps to an 8px grid when released.

Settings live in `%APPDATA%\FrostWidgets` (`config.toml`, `notes.txt`).

## Build

Needs Rust (GNU toolchain) and `windres` on PATH (for the icon and version info).

```
.\build-setup.ps1
```

This builds `frost.exe`, compresses it into the installer and writes `dist\FrostWidgets-Setup-<version>.exe`.
For quick development runs: `cargo build --release --bin frost`, then `target\release\frost.exe`.

`frost.exe --snapshot preview.png` renders all widgets, the reminders editor and every settings tab to PNGs and exits.

## Why it's light

- Event-driven: nothing redraws unless what it shows changed; no render loop.
- Glass blur is computed once from the wallpaper, not every frame.
- Software Direct2D (no GPU device), one process, one UI thread.
- Pauses when the display is off, the PC is locked, or a maximized/fullscreen app covers the widgets.
- Settings and reminders windows exist only while open.
- Media info comes from Windows media events; weather uses the built-in `curl.exe` every 30 minutes.
