# Changelog

What changed in each version of OxShot. The newest version is at the top.
New entries go under "Unreleased"; `dev/release.sh` turns that section into
the next version.

## Unreleased

## 0.1.1 - 2026-10-14

### Fixed

- Capturing from the tray menu no longer freezes the screen with the menu still
  on it: captures started from the menu wait until the panel has closed it.

## 0.1.0

The first public version.

### Capture

- A frozen screen within a few dozen milliseconds: hover a window and click to take
  it, or drag a region; a pixel loupe shows coordinates and color, arrow
  keys adjust to the pixel.
- Region, last region, one monitor or all monitors, with or without the
  mouse pointer.
- Delayed capture with a small countdown that takes no focus, so open menus
  stay open and end up in the picture.
- Screen color picker: a round loupe with a pixel grid, a click copies
  `#RRGGBB`.
- Every capture is saved to `~/Pictures/OxShot` and put on the clipboard as
  image, file and path at once.

### Editor

- Annotations stay editable objects, also later: a project file is saved
  next to every image.
- Arrows, rectangles, ellipses, polygons, freehand, text, callouts, numbered
  steps, highlighter, curly braces, images and stamps, blur and pixelate, and
  a magnifier.
- Non-destructive crop, resize, cut out a strip with a straight, jagged or
  wavy edge, and frames and effects: shadow, rounded corners, border, torn
  edges, reflection, grayscale, rotation.
- Copy (Ctrl+C), copy and close (Ctrl+Enter), save as PNG, JPEG, WebP, BMP,
  TIFF or `.oxshot` project; paste images from the clipboard.

### Around it

- Runs in the background with a tray icon and global hotkeys, changeable in
  the settings; `oxshot capture`, `oxshot pick` and friends from the command
  line.
- History of all captures as a grid: open, copy, rename, move to the trash,
  import by drag and drop.
- Autostart and a menu entry that keep pointing at the running program, also
  for an AppImage.
- Light, dark or following the desktop; English, German, Spanish, French,
  Portuguese (Brazil), Russian and Simplified Chinese.
