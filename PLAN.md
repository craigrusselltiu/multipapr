# Multipapr

Build a Windows wallpaper editor and a quiet, per-user wallpaper host in Rust.

1. Read active display rectangles in native pixels and match each display to its EDID. Convert the active panel area to millimetres, account for rotation, and use Windows arrangement to infer which edges touch. Keep manual physical placement and size corrections for missing or inaccurate hardware data.
2. Share one physical coordinate system between the editor and renderer. At native scale, one media pixel equals one primary-display pixel. Offer a cover fit, dragging, proportional resizing and numeric calibration of space between panels. The bezel and any air gap consume part of the image.
3. Build a compact dark editor with a media canvas, outlined displays, a source picker and a small placement inspector. Use a cached still for editing video. Apply a whole layout together and retain it locally.
4. Use a separate lightweight host attached to Explorer behind the icons. Decode the source once with mpv and map it onto all monitor rectangles in one GPU shader. Loop silently, prefer hardware decoding, avoid continuous work for photos and pause when the desktop is locked or fully covered by a fullscreen app.
5. Make the system tray the app's home. Start quietly at sign-in with one tray icon and the saved wallpaper. Left-click the icon to open or focus the editor. Closing the editor window returns to the tray and keeps playback running. Right-click the icon and choose **Close** to stop playback, remove the tray icon and exit Multipapr. Preserve the sign-in preference for the next session. Restore the icon and wallpaper after Explorer restarts. Keep user media and machine-specific settings outside Git.
6. Verify geometry with deterministic tests, build and exercise the release on the actual displays, then apply the supplied UHD clip and publish the source.

## Editor refinement

Use one draggable app bar with native window actions and rounded desktop-window corners. Keep the wallpaper visible in a large canvas. The sidebar holds zoom, canvas fit and the wallpaper and display tools. Middle-drag or drag empty space to pan. Window actions and the settings cog share a single title bar. Keep image placement, physical display calibration and the temporary camera transform independent.
