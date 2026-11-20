# Multipapr

A Windows wallpaper editor for desks with mixed display sizes, densities and orientations. Play one silent, looping video across the desktop or use a static image. The editor and wallpaper host share a layout measured in millimetres.

## Install

Install Rust and FFmpeg with both `ffmpeg.exe` and `ffprobe.exe` on PATH. From the repository, run:

```powershell
.\install.ps1
```

The installer builds the release, places it in `%LOCALAPPDATA%\Multipapr` and adds **Multipapr** to Start. On x64 Windows it downloads a checksum-verified portable mpv build if needed. An existing mpv on PATH also works. FFmpeg is used when importing media and generating the editor thumbnail.

## Arrange a wallpaper

1. Open **Multipapr**, choose a file or drop it onto the canvas. The bright regions show what each panel displays. The dim region shows the rest of the image.
2. **Native size** maps a source pixel to the primary screen's pixel pitch. A 3840 × 2160 source is twice the width and height of a 1920 × 1080 primary display. **Cover** fills the entire physical arrangement. Drag the picture to move it and drag its bottom-right handle to resize, or enter a scale percentage. Double-click the canvas to frame the whole composition again.
3. Select a display in the inspector. Panel width and height are the lit dimensions in its current orientation. For side-by-side displays, enter the distance between the lit edges in **Horizontal gap**. **Top offset** is the selected top edge relative to the indicated neighbour, with a negative value above that neighbour. Stacked displays use **Vertical gap** and **Left offset**. Include both bezels and any air space in the gap.
4. Click **Apply wallpaper**. Close the editor while the background keeps running. **Start at sign-in** restores the saved layout under your account. **Stop wallpaper** reveals the Windows wallpaper underneath. Turn off sign-in startup as well to keep it stopped after a reboot.

Multipapr reads the panel dimensions from EDID and falls back to a 96 DPI estimate for panels without usable size information. Windows supplies the initial alignment. Physical gap defaults to zero. Panels with inaccurate firmware dimensions can be corrected in the inspector.

## Playback

The wallpaper host attaches a child window behind Explorer's desktop icons. One mpv instance decodes the media and a GPU shader maps it to each display. Hardware decoding is requested when available. Video plays at the source cadence with interpolation, audio and network caching disabled. A still image holds one frame. The editor loads a bounded, cached preview. The host pauses on a locked desktop or when the foreground window covers the complete desktop, and restores the surface after Explorer or monitor changes.

Media stays at its original path. Settings, cached previews and diagnostic logs live in `%LOCALAPPDATA%\Multipapr`. Keep the selected file accessible at sign-in. Explorer's wallpaper window hierarchy is an undocumented Windows interface and can change across Windows versions.

## Command line

```powershell
multipapr.exe --apply "C:\Media\wallpaper.mp4"
multipapr.exe --displays
multipapr.exe --background
multipapr.exe --stop
```

`--apply` fits a new file to the calibrated display layout. On first use it also enables startup. Run without arguments to open the editor. Build and test with `cargo build --release --locked` and `cargo test --release --locked`.

[High-level design](PLAN.md) · [mpv](https://mpv.io/) · [Windows mpv builds](https://github.com/shinchiro/mpv-winbuild-cmake)
