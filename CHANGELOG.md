# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## v0.3.7

- Gallery uses uncropped Wallhaven thumbnails so portrait and ultrawide hover previews show the full image.

## v0.3.6

- Gallery hover previews fit the wallpaper's aspect ratio within the editor. Wallpapers with the same aspect ratio share the same preview size.

## v0.3.5

- Replaced the animated wallpaper playback control with a play/pause icon button beside **Fill** and **Fit**.
- Gallery hover previews now size to the wallpaper’s resolution and aspect ratio, within the editor window. Animated previews preserve portrait and ultrawide framing.

## v0.3.3

- Added **Pause** / **Play** beside **Fill** and **Fit** for animated wallpapers. Pausing stops the video player and displays the first frame as a static wallpaper with no GPU use.
- Added **Set Windows background** in **Settings → General** (off by default) to mirror each display’s wallpaper—or first frame for animated wallpapers—to Windows’ underlying desktop background, replacing files in place.
- Moved wallpaper clearing to an inline **×** button inside the file picker box, and removed the Playback status section from **Advanced**.
- Gallery search now runs automatically one second after typing stops.
- Increased the default editor window size (`1160 × 760`), aligned the default sidebar width with the title-bar divider, evened out the padding around the divider, and kept Settings subtext single-line with roomier vertical padding.

## v0.3.2

- Dragging a wallpaper snaps its edges to nearby display edges, and dragging a display snaps it to the wallpaper and to other displays. Hold <kbd>Shift</kbd> while dragging to place freely.
- The editor opens centered on the main display.
- Tooltips have more padding and keep clear of the window edge and of the dialog or popup they belong to.
- Lower GPU use while playing. Displays that show the wallpaper at or below its native resolution skip the upscaling filter, Catmull-Rom upscaling samples nine texels instead of sixteen, and Spline36 and Lanczos compute their weights once per row and column.

## v0.3.1

- Gallery uses MotionBGs and DesktopHut for animated wallpapers and Wallhaven's Hot feed for stills. Unfiltered results alternate between all three sources.
- Wallhaven text searches use the site's relevance ordering and page through its matching results.
- Search and filters share the Gallery header. The filter panel stays inside the dialog, and hover previews open sooner at a smaller size.
- Gallery prepares up to two pages per source in the background and reveals the complete batch in source order near the bottom.
- DesktopHut checks direct download links with up to eight concurrent requests, overlapping those checks with the next listing page.
- The default feed follows MotionBGs' homepage link into its additional pages.
- Cards show the resolution, media type, and source. Missing wallpaper messages use simpler wording.
- Switching profiles preserves the current Multi-mode setting.

## v0.3.0

- Added **Gallery** beside Profiles, with live picks from MotionBGs and DesktopHut plus Wikimedia Commons and NASA wallpapers. Click a card to apply it with **Fit**, pause over one for a larger preview, or right-click to choose a display or delete a saved copy.
- The **Downloaded** filter shows saved wallpapers offline, including files imported from your computer. Settings shows the space used and lets you clear saved copies.
- Manually selected wallpapers use **Fit** by default. The Gallery cleanup in Settings uses a red **Delete** button with a separate confirmation.
- Gallery resizes with the editor and blurs the editor behind it. The Multipapr title bar is slightly thinner.
- Gallery and Settings fade in and out along with their blurred backdrop.
- Gallery has a larger hover preview, compact right-click menus, and a checkmark on downloaded cards. **Load more** appends results below the current cards.
- Gallery opens on animated wallpapers. The filter popup offers Still, Animated, Downloaded, and resolution options.
- Browsing Gallery displays previews in memory. Gallery searches run while the popup is open, and wallpapers are saved when applied.
- Gallery checks for an original DesktopHut MP4 before showing a card and searches Wikimedia Commons for still images.
- In Span mode, Gallery closes one second after the selected wallpaper is downloaded. The loading circle beside Multi-mode shows when a new wallpaper is loading in either mode.
- **Clear** beside Fill and Fit fades the selected display or spanning wallpaper to black. Deleting a saved wallpaper or clearing the Gallery also clears its assignments.
- Multi-mode fits the current Span wallpaper to new displays and displays whose saved wallpaper was deleted. Cleared displays show **No wallpaper**.

## v0.2.0

- Multi-mode switches preload the replacement wallpapers and fade them in together after their first rendered frames. A loading circle beside the switch shows when the transition is in progress.
- Added playback actions for fullscreen apps and Windows Energy Saver. Advanced shows the assigned file and playback status for the selected display.
- Settings expands to fit all sections and scrolls in shorter windows.
- Connecting or disconnecting a monitor preserves the spanning wallpaper’s position and scale on the remaining screens. A saved framing in the active profile takes precedence for that exact monitor setup.

## v0.1.5

### Added

- **Undo** and **Redo** buttons in the title bar, with <kbd>Ctrl</kbd>+<kbd>Z</kbd>, <kbd>Ctrl</kbd>+<kbd>Y</kbd> and <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd>. Typing into one field counts as a single step.
- **Multi-mode**, a switch at the top of the sidebar that gives each display its own wallpaper. Selecting a display shows its wallpaper in full behind the canvas, while every other display shows its own wallpaper cropped to it. Each display plays in its own mpv instance.
- **Profiles**, from the bookmark in the title bar. A profile is one wallpaper setup that remembers its framing for every combination of connected monitors. When monitors are connected, disconnected or switched between Duplicate and Extend, the active profile brings back the framing it last had on that combination, or fits itself to monitors it hasn't seen. Edits save into the active profile automatically. **Default** always exists and can't be deleted. Display calibration is shared by every profile.
- Resize handles on the middle of the right and bottom edges of the wallpaper and the selected display, which change only the width or height. Displays also get a corner handle.
- **Maintain ratio** switch in **Advanced**, on by default, so changing a panel's width or height scales the other.
- Clicking selects the top display under the pointer. Repeated clicks cycle through overlapping items. Clicking empty canvas clears the selection and previews each display’s crop in multi-mode.
- Duplicated displays are detected, and the editor shows which screens each display is duplicated on.

### Changed

- Wallpaper positions are now stored in desk millimetres. Existing layouts keep their framing when converted on first load.
- Display positions and wallpaper pixel size survive a change of primary monitor. When the connected set changes, the active profile restores or fits the span while per-display wallpapers keep their framing.
- A newly connected display is selected in the editor. If its default spot would overlap a display you've placed, it moves beside that display instead.
- The wallpaper host reacts to display changes immediately instead of within 4 seconds.
- Overlapping displays are easier to work with. The selected display is drawn on top, every outline stays visible, overlapping areas are hatched, clicking an overlap steps through the displays under the pointer, and the sidebar warns about the overlap. Number badges no longer cover each other.
- The wallpaper has an accent outline while it's selected.
- One sidebar replaces the wallpaper and display tabs: the wallpaper above the displays, or in multi-mode the displays first with the selected display's wallpaper below. It's resizable and scrolls only when its contents overflow.
- The chosen file shows in a field with an open button, above **Fill** and **Fit**.
- Zoom and **Reset scale** moved to the top-right corner of the canvas.
- **Reset displays** replaces **Reset display**, sits outside **Advanced**, and restores every display's detected size and position.
- **Advanced** no longer shows which display the offsets are relative to.

### Fixed

- Display detection skips inactive monitor entries on an output.
- Saving no longer fails with "Access is denied" when another process briefly has the layout file open, and a save error clears after the next successful save.
- Changing or losing one display's file in multi-mode leaves the other videos playing. Live framing updates go to the player for the matching file and display geometry.
- If Explorer hides or moves a wallpaper window, Multipapr restores it to its monitor and keeps its video playing.
- Each display keeps its own pending file selection while previews load. Switching profiles checks the files used on connected screens.
- Docking, closing the editor during a monitor change, and reopening after an interrupted profile save preserve the current files and restore the span framing for the connected screens. A duplicated output has its own framing.
- Undo restores saved monitor framings when a wallpaper file is replaced. Creating a profile starts a fresh undo history.
- Profile saves retry after a write failure. An unreadable layout is reported before it can be overwritten.

## v0.1.4

### Added

- **Close to system tray** option in Settings (enabled by default). Turning it off makes closing the editor exit Multipapr completely, including the wallpaper and tray icon.
- **Version indicator and in-app updater** in Settings. Opening Settings checks GitHub for a newer release; when one is available, **Update** downloads, verifies, replaces and relaunches `multipapr.exe` in place.

### Changed

- Redesigned the Settings dialog with grouped cards, toggle switches and descriptions.

### Fixed

- System tray right-click menu now opens at the cursor in front of the taskbar and hidden-icons panel.

## v0.1.3

### Added

- **Upscaling** option in the wallpaper sidebar: None, Catmull-Rom (default), Spline36, Lanczos and EWA Lanczos Sharp, using mpv's filter definitions. Changes apply to the running wallpaper live.

## v0.1.2

### Changed

- First-time setup is much faster. The bundled mpv is stored as zstd and unpacks in about half a second instead of 2.5 seconds.
- Setup starts in the background as soon as the app opens, and mpv runs once ahead of time, so a new user's first wallpaper loads in under half a second.
- Importing a video starts mpv once instead of twice.
- The sidebar shows "Running first-time setup…" while setup runs.

### Added

- MIT license for Multipapr, plus third-party notices and the GPL text for the bundled mpv.

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
