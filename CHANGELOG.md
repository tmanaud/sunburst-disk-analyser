# Changelog

What changed in each release. Downloads are on the [releases page](https://github.com/tmanaud/sunburst-disk-analyser/releases).

## 1.1.0 - 2026-11-02

### New

- Several items at once: Shift-click selects a range of rows, Ctrl-click (⌘-click on macOS) or Alt-click adds or removes one, in the outline columns, Biggest Files and the search results. Shift with the up or down arrow extends the selection; Escape goes back to a single row.
- Several folders selected in a column show the biggest files they hold, largest first, in the next pane. Click one to preview it.
- Right-click a selection of several rows to tick or untick all of them, remove, move or copy them, or copy their paths.
- Shift-click a checkbox to tick or untick every row from the last checkbox clicked.
- Click a file in the outline for a preview pane with a thumbnail: pictures on every system, PDFs and MOV or MP4 videos on macOS, any video on every system once ffmpeg is installed. Double-click it to open it.
- Back and forward through the folders you opened: a two-finger swipe at the edge of the columns, the side buttons of a mouse, ⌘[ and ⌘] on macOS, Ctrl+[ and Ctrl+] or Alt with the left or right arrow on Windows and Linux. The arrow keys walk the columns.
- The toolbar shows the disk as one bar: the scanned location, the rest of the used space and the free space, and how much space removing the ticked items gives back.
- Search filters: file type, minimum size and modification date, with or without a name.
- An estimated progress bar while scanning.
- The home folder can be scanned at launch (a new setting). On macOS this waits until Full Disk Access is granted, so opening the app never sets off permission prompts.
- The app tells you when a newer release is out and can download it to your Downloads folder, checked against the release checksums. It never replaces itself. The check at launch can be turned off in Settings.
- Drag the edge of the outline columns or of the lists to resize them.
- Settings shows whether Full Disk Access is granted; Help shows the version and the project page.

### Changed

- The chart is centred on the folder that holds the selection. Click a segment to select it; a double-click no longer dives into it.
- The context menu says Tick and Untick instead of Select and Deselect.

### Safety

- Remove on Linux moves the item to the Trash of its own volume, or is refused: it never copies then deletes.
- Copy, move and scan never follow a symbolic link swapped in while they run (macOS and Linux).
- Every item is checked against the scan and the review right before it is touched. Mounted volumes and unreadable folders cannot be selected. Remove on Windows is refused on drives without a Recycle Bin.
- Copies are built in a private folder and never replace an existing name. CSV reports and settings are written aside, then put in place.
- A scan keeps at most 40 million items, and 4 million per folder; beyond that the folder is flagged as partly unread instead of exhausting memory.

## 1.0.0 - 2026-10-12

First release: fast parallel scan, sunburst chart, folders outline, biggest files, search, and review before moving to the Trash, moving or copying. macOS, Windows and Linux, in eight languages.
