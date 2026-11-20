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
2. Use the image icon to edit the wallpaper. **Fill** centres the image at native size. At that scale, a 3840 × 2160 image is twice the width and height of a 1920 × 1080 primary display. **Fit** beside **Fill** covers the physical arrangement. Drag the image to move it or drag its bottom-right handle to resize it.
3. Use the monitor icon to select and move a display. Multipapr starts from the Windows arrangement and remembers your physical positions by monitor ID. The wallpaper updates during the drag and saves on release. Open **Advanced** for panel dimensions and offsets in millimetres. Positive horizontal and vertical offsets move right and down. For adjacent displays, zero on the joining axis puts their lit edges together. Include both bezels and any air gap in your measurement.
4. Drag empty canvas or middle-drag anywhere to pan. Hold **Ctrl** while dragging to move horizontally, or **Ctrl + Shift** to move vertically. The wheel zooms around the pointer. **Reset scale** beside the zoom buttons frames the whole canvas.
5. Open the settings cog to change **Start at sign-in**. Close the editor to keep the wallpaper running in the tray. Click the tray icon to reopen the editor or right-click it and choose **Close** to quit.

Multipapr reads panel dimensions from EDID. When the reported size conflicts with the preferred pixel aspect, it estimates the shorter edge from that aspect. Panels without usable size information use a 96 DPI estimate. Windows supplies the initial alignment and the physical gap starts at zero. You can correct all estimates in the inspector.

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

`--apply` fits a new file to the calibrated display layout. On first use it also enables startup. Run without arguments to open the editor, or with `--background` to start in the tray. `--stop` quits the tray and wallpaper host. Build and test with `cargo build --release --locked` and `cargo test --release --locked`.

[High-level design](PLAN.md) · [mpv](https://mpv.io/) · [Windows mpv builds](https://github.com/shinchiro/mpv-winbuild-cmake)
