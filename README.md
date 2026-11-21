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

## Summary

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/readme/diagram-dark.png">
    <img src="assets/readme/diagram-light.png" alt="Pixel spanning versus physical spanning" width="100%">
  </picture>
</p>

1. **Pick a wallpaper.** Drop or browse for a video (MP4, MOV, MKV, WebM) or image (PNG, JPEG, WebP, BMP).
2. **Frame it.** Drag to move, pull a corner to resize, or use **Fill**, **Fit** and **Upscaling** in the sidebar.
3. **Align your displays.** Switch to the monitor tool and drag each screen to match your desk, or enter exact millimetre gaps (including bezels) under **Advanced**. Changes apply live.
4. **Close the window.** Multipapr keeps playing from the tray by default and starts at sign-in.

<table>
  <tr><td><b>Canvas</b></td><td>Drag empty space or middle-drag to pan. Scroll to zoom. <b>Reset scale</b> reframes.</td></tr>
  <tr><td><b>Lock axis</b></td><td>Hold <kbd>Ctrl</kbd> for horizontal, <kbd>Ctrl</kbd>+<kbd>Shift</kbd> for vertical.</td></tr>
  <tr><td><b>Tray</b></td><td>Click to reopen. Right-click → <b>Close</b> to stop the wallpaper.</td></tr>
  <tr><td><b>Settings</b></td><td>Toggle <b>Start at sign-in</b>, <b>Close to system tray</b> or update Multipapr from the cog icon.</td></tr>
</table>

## Lightweight

> [!NOTE]
> Playback uses [mpv](https://mpv.io/), which is built into the exe and unpacked once on first use.
>
> A single mpv instance decodes the video once, on the GPU where possible, and one shader maps it to every display. Audio, interpolation and network caching are off. Playback **pauses** when the desktop is locked or a fullscreen window covers it, and it recovers after Explorer restarts or displays change. A still image renders once and then stays idle.

## CLI

```powershell
multipapr.exe --apply "D:\Wallpapers\loop.mp4"   # fit a file to your displays and start it
multipapr.exe --background                        # start in the tray
multipapr.exe --stop                              # stop the wallpaper and quit
multipapr.exe --displays                          # print detected displays as JSON
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

</details>

## License

[MIT](LICENSE). The release exe bundles [mpv](https://mpv.io/), which is GPL-licensed. See [third-party notices](THIRD-PARTY-NOTICES.md).
