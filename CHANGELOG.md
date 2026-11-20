# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## v0.1.2

### Changed

- First-time setup is much faster. The bundled mpv is stored as zstd and unpacks in about half a second instead of 2.5 seconds.
- Setup starts in the background as soon as the app opens, and mpv runs once ahead of time, so a new user's first wallpaper loads in under half a second.
- Importing a video starts mpv once instead of twice.
- The sidebar shows "Running first-time setup…" while setup runs.

## v0.1.1

### Changed

- mpv is now built into `multipapr.exe` and unpacked locally on first use, so playback works offline with no extra download.
- The installer no longer downloads mpv and removes the copy that earlier versions downloaded.

## v0.1.0

### Added

- Editor showing each display's true physical size against the wallpaper at its native scale, with move and resize.
- Physical display placement by drag or by millimetre offsets under **Advanced**.
- Looping silent video and still-image wallpapers spanned across all displays by a single mpv instance and GPU shader.
- Pauses on a locked desktop or under a fullscreen window, and recovers after Explorer or display changes.
- Tray icon and start at sign-in.
- Standalone `multipapr.exe` release that downloads a checksum-verified portable mpv on first use. FFmpeg is no longer required.
- One-command installer: `irm https://raw.githubusercontent.com/craigrusselltiu/multipapr/main/install.ps1 | iex`.
