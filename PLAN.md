# Multipapr

Build a Windows wallpaper editor and a quiet, per-user wallpaper host in Rust.

1. Read active display rectangles in native pixels and match each display to its EDID. Convert the active panel area to millimetres, account for rotation, and use Windows arrangement to infer which edges touch. Keep manual physical placement and size corrections for missing or inaccurate hardware data.
2. Share one physical coordinate system between the editor and renderer. At native scale, one media pixel equals one primary-display pixel. Offer a cover fit, dragging, proportional resizing and numeric calibration of space between panels. The bezel and any air gap consume part of the image.
3. Build a compact dark editor with a media canvas, outlined displays, a source picker and a small placement inspector. Use a cached still for editing video. Apply a whole layout together and retain it locally.
4. Use a separate lightweight host attached to Explorer behind the icons. Decode the source once with mpv and map it onto all monitor rectangles in one GPU shader. Loop silently, prefer hardware decoding, avoid continuous work for photos and pause when the desktop is locked or fully covered by a fullscreen app.
5. Restore the saved layout at user sign-in, recover after Explorer or monitor changes, and expose a stop action. Keep user media and machine-specific settings outside Git.
6. Verify geometry with deterministic tests, build and exercise the release on the actual displays, then apply the supplied UHD clip and publish the source.
