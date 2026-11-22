<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/readme/logo-dark.png">
    <img src="assets/readme/logo-light.png" alt="Multipapr" width="380">
  </picture>
</p>

<p align="center">
  <b>One wallpaper. Every monitor. Lined up like a single window onto your desk.</b><br>
  Live MP4 and still-image wallpapers for Windows.
</p>

<p align="center">
  <a href="https://github.com/craigrusselltiu/multipapr/releases/latest"><img src="https://img.shields.io/github/v/release/craigrusselltiu/multipapr?style=flat-square&color=c1bbfc&label=download" alt="Download"></a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-2c2c32?style=flat-square" alt="Windows 10 and 11">
  <img src="https://img.shields.io/badge/built%20with-Rust-2c2c32?style=flat-square" alt="Rust">
</p>

---

Windows spans wallpapers by **pixels**, which breaks scale and alignment across mixed monitors and bezels. Multipapr maps by **physical size** instead, so the image continues cleanly from one screen to the next.

<table>
  <tr>
    <th>Multipapr</th>
    <th>Live Displays</th>
  </tr>
  <tr>
    <td><img src="assets/readme/demo-app.gif" alt="Resizing the wallpaper and moving display 2 in the Multipapr editor"></td>
    <td><img src="assets/readme/demo-irl.gif" alt="The same wallpaper playing across a landscape and a portrait monitor"></td>
  </tr>
</table>

## Install

Download [`multipapr.exe`](https://github.com/craigrusselltiu/multipapr/releases/latest/download/multipapr.exe) and run it, or run one command in PowerShell to install it with a Start menu entry:

```powershell
irm https://raw.githubusercontent.com/craigrusselltiu/multipapr/main/install.ps1 | iex
```

## Goal

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/readme/diagram-dark.png">
    <img src="assets/readme/diagram-light.png" alt="Pixel spanning versus physical spanning" width="100%">
  </picture>
</p>

The problem Multipapr tries to solve is the visible jump where a wallpaper crosses from one monitor to the next. A circle split across two screens should still look like one circle.

## Features

<table>
  <tr><td><b>Multi-mode</b></td><td>Give each display its own wallpaper. Select a display to see its full wallpaper and adjust its framing. Click empty canvas to preview every wallpaper cropped to its display. Open <b>Advanced</b> below Wallpaper for exact display sizes and offsets.</td></tr>
  <tr><td><b>Profiles</b></td><td>The bookmark in the title bar lists your wallpaper setups. Connecting or disconnecting a monitor keeps the same crop on the remaining screens. If the active profile has a saved framing for that exact monitor combination, Multipapr restores it. Edits save automatically.</td></tr>
  <tr><td><b>Gallery</b></td><td>Browse MotionBGs, DesktopHut, Wikimedia Commons and NASA. Click a wallpaper to apply it, hover for a larger preview, or right-click for display and delete options. Use <b>Downloaded</b> to browse saved and imported wallpapers offline. Files are stored in <code>%LOCALAPPDATA%\Multipapr\Wallpapers</code>.</td></tr>
  <tr><td><b>Lock axis</b></td><td>Hold <kbd>Ctrl</kbd> for horizontal, <kbd>Ctrl</kbd>+<kbd>Shift</kbd> for vertical.</td></tr>
  <tr><td><b>Tray</b></td><td>Click to reopen. Right-click → <b>Close</b> to stop the wallpaper.</td></tr>
  <tr><td><b>Settings</b></td><td>Toggle <b>Start at sign-in</b>, <b>Close to system tray</b> or update Multipapr from the cog icon.</td></tr>
</table>

## Playback

> [!NOTE]
> Playback uses [mpv](https://mpv.io/), which is built into the exe and unpacked once on first use.

Multipapr decodes a shared video once, using the GPU when possible. Still images render once and then idle.

**Multi-mode** uses one player per display. In **Settings → Playback**, **Fullscreen game detected** covers fullscreen foreground apps, including borderless games. **Battery saver** follows Windows Energy Saver. Both settings can keep playing (on visible displays for fullscreen apps), pause all wallpapers, or stop to free memory. Multipapr pauses playback when Windows locks and resumes automatically when conditions clear.

## CLI

```powershell
multipapr.exe --apply "D:\Wallpapers\loop.mp4"
multipapr.exe --background
multipapr.exe --stop
multipapr.exe --displays
```

Settings, previews and logs live in `%LOCALAPPDATA%\Multipapr`.

<details>
<summary><b>Build from source</b></summary>

```powershell
git clone https://github.com/craigrusselltiu/multipapr
cd multipapr
cargo test --release --locked
.\install.ps1 -FromSource
```

</details>

<details>
<summary><b>Known limits</b></summary>

- Multipapr places its wallpaper window behind the desktop icons using a part of Explorer that Windows doesn't document, so a future Windows update could break it.
- If a monitor reports no physical size, Multipapr assumes 96 DPI. You can correct it under **Advanced**.
- Screens in Windows' **Duplicate** mode show the same pixels, so the wallpaper is sized for the first screen on that output.

</details>

## License

[MIT](LICENSE). The release exe bundles [mpv](https://mpv.io/), which is GPL-licensed. See [third-party notices](THIRD-PARTY-NOTICES.md).
