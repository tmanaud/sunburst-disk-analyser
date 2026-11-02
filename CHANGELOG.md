# Changelog

What changed in each release. Downloads are on the [releases page](https://github.com/tmanaud/sunburst-disk-analyser/releases).

## 1.1.0 - 2026-11-02

### New

- Select several rows and act on them together; Shift-click ticks a range of checkboxes.
- Select several folders to list their biggest files together.
- Preview pictures, PDFs and videos; formats vary by system.
- Go back and forward with gestures, mouse buttons or keyboard shortcuts.
- Disk bar in the toolbar: used and free space, and what the ticked items would free.
- Search filters: type, size and date, with or without a name.
- Estimated scan progress.
- Scan the home folder at launch; on macOS once Full Disk Access is granted.
- Update check at launch, which can be turned off. Downloads are verified; installing stays manual.
- Resize columns and lists.
- Settings shows Full Disk Access; Help shows the version and project page.

### Changed

- The chart is centred on the folder that holds the selection; double-click no longer dives in.
- The context menu says Tick and Untick.

### Safety

- Linux removes to the volume's own Trash; Windows refuses drives without a Recycle Bin.
- macOS and Linux never follow a folder swapped for a symbolic link.
- Items are checked again right before any operation. Mounted volumes and unreadable folders cannot be selected.
- Copies never replace an existing name and appear only once complete.
- CSV reports and settings are replaced whole.
- Scans stop at 40 million items, 4 million per folder, and flag what was left unread.

[Details](https://github.com/tmanaud/sunburst-disk-analyser/blob/main/docs/DETAILS.md)

## 1.0.0 - 2026-10-12

First release: fast parallel scan, sunburst chart, folders outline, biggest files, search, and review before moving to the Trash, moving or copying. macOS, Windows and Linux, in eight languages.
