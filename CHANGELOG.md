# Changelog

What changed in each release. Downloads are on the [releases page](https://github.com/tmanaud/sunburst-disk-analyser/releases).

## 1.1.0 - 2026-11-01

### New

- Several items at once: Shift-click selects a range of rows, Ctrl-click (⌘-click on macOS) or Alt-click adds or removes one, in the outline columns, Biggest Files and the search results. Shift with the up or down arrow extends the selection; Escape goes back to a single row.
- Several folders selected in a column show the biggest files they hold, largest first, in the next pane. Click one to preview it.
- Right-click a selection of several rows to tick or untick all of them, remove, move or copy them, or copy their paths.
- Shift-click a checkbox to tick or untick every row from the last checkbox clicked.
- The toolbar shows the disk as one bar: the scanned location, the rest of the used space and the free space, and how much space removing the ticked items gives back.
- Search filters: file type, minimum size and modification date, with or without a name.
- An estimated progress bar while scanning.
- The home folder can be scanned at launch (a new setting). On macOS this waits until Full Disk Access is granted, so opening the app never sets off permission prompts.
- The app tells you when a newer release is out and can download it to your Downloads folder, checked against the release checksums. It never replaces itself. The check at launch can be turned off in Settings.
- Drag the edge of the outline columns or of the lists to resize them.
- Video thumbnails for MKV, WebM and other formats through an installed ffmpeg, on every system.
- Settings shows whether Full Disk Access is granted; Help shows the version and the project page.

### Changed

- The context menu says Tick and Untick instead of Select and Deselect.

### Fixed

- Alt with the left or right arrow goes back or forward on Windows and Linux, also while browsing the outline.
- The preview no longer waits 15 seconds on videos that Quick Look cannot read.
- Settings notices Full Disk Access granted while it is open.
- Search results with a size filter are updated after a removal.

### Safety

- Remove on Linux moves the item to the Trash of its own volume, or is refused: it never copies then deletes.
- Copy, move and scan never follow a symbolic link swapped in while they run (macOS and Linux).
- Every item is checked against the scan and the review right before it is touched. Mounted volumes and unreadable folders cannot be selected. Remove on Windows is refused on drives without a Recycle Bin.
- Copies are built in a private folder and never replace an existing name. CSV reports and settings are written aside, then put in place.

## 1.0.0 - 2026-10-12

First release: fast parallel scan, sunburst chart, folders outline, biggest files, search, and review before moving to the Trash, moving or copying. macOS, Windows and Linux, in eight languages.
