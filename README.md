# OledHelper
Windows utility to complement OLED TV/monitors. You can adjust black level per refresh rate with built-in calibration, a taskbar overlay dimmer and customizable hotkeys.

![OledHelper](assets/crush_sdr.jpg)

---

## Features

**Black Crush Tweak** - Corrects HDR/SDR near-black by applying a custom gamma ramp (Reinhard shadow curve) restoring shadow detail. Includes a live Direct3D HDR/SDR calibration panel, per-refresh-rate profiles.

**Taskbar Dimmer** - Places a dark semi-transparent overlay on the taskbar to reduce static brightness. Fades out on hover. Automatically hides when a fullscreen app is active, the taskbar is set to auto-hide. Dim level and fade timings are adjustable.

**Settings** - Adjust various Windows settings like power/screensaver options.

**Hotkeys** - Global hotkeys to toggle/adjust features.

---

## Requirements

- Windows 10 or 11 (x64)
- DirectX 11 GPU

---

## Installation

No installer. Download `OledHelper.exe` from [Releases](../../releases) and run it. Settings are saved to `%APPDATA%\OledHelper\OledHelper.ini`.

To start with Windows, enable the checkbox in the app. Closing the window minimizes to tray, to exit the app select exit in the app or on the tray menu.

---

## Building

Requires the [Rust toolchain](https://rustup.rs/) targeting `x86_64-pc-windows-msvc`.

```
cargo build --release
```

Place tab icons in `assets/icons/` before building (`tab_crush.png`, `tab_dimmer.png`, `tab_hotkeys.png`, `tab_debug.png`, `tab_about.png`).

---

## License

Copyright (C) 2026 MrHuntsman

Licensed under the [GNU General Public License v3.0](LICENSE). You are free to use, modify, and distribute this software, but any distributed modifications must also be released under GPL-3.0.
