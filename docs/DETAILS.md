# Controls and details

## Explore

- Scans run in parallel, show live results and estimated progress, and can be stopped.
- Scanning the home folder at launch is on by default (Settings). On macOS it waits for Full Disk Access. Unreadable folders are counted and reported.
- The chart is centred on the folder that holds the selection. One ring per folder level. Hover for size, file count and date; click to select; click the centre to go up.
- Folders Outline: click a folder to open the next column, a file to preview it; double-click a file to open it. Small and hidden items are grouped. Drag column or list edges to resize them.
- Shift-click selects a range. Ctrl-click (⌘ on macOS) or Alt-click adds or removes one row. Several folders selected in a column show their biggest files in the next pane. Right-click a selection to act on all of it.
- Shift-click a checkbox to tick or untick a range. Shift+Up/Down extends the selection; Escape goes back to one row.
- Scroll sideways or use the arrow keys to move through the columns. Back and forward: mouse side buttons, a two-finger swipe at the column edge, ⌘[ / ⌘] (Ctrl on Windows and Linux), or Alt+Left/Right on Windows and Linux.
- Search by name, type (video, picture, audio, document, archive), minimum size or modification date. Filters work without a name.
- The disk bar shows the scanned location, the rest of the used space and the free space, and what the ticked items would free. Whole-disk scans also show free and unattributed space beside your files.
- Also: favourite folders, drag and drop of folders, CSV export, show in Finder / Explorer / file manager, copy path.
- Light, dark or automatic appearance. English, French, German, Spanish, Italian, Russian, Ukrainian and Simplified Chinese.

## Previews

Pictures on every system; PDFs and MOV/MP4 videos on macOS. Install [ffmpeg](https://ffmpeg.org) (not bundled) for other videos on any system.

Quick Look is macOS only; other systems open the file with its default app. File icons are generic.

## Sizes and scan limits

- macOS and Linux count the space allocated on disk: sparse files and cloud-only files count only what they use. A file with several hard links counts once, under the first name found; its other names show as empty. Scan again after removing that name.
- Windows counts file length. OneDrive online-only files count as zero; each hard link counts.
- Scans do not follow symbolic links or Windows junctions. On macOS and Linux they stay on the scanned file system.
- Scanning asks macOS not to download iCloud files.
- At most 40 million items per scan and 4 million per folder; beyond that, the folder is marked as partly unread.
- APFS clones and snapshots share blocks, so your files can add up to more or less than the used space macOS reports. Whole-disk scans show the difference as **Other used space**. Purgeable space is not shown separately.

## File operations

- Linux: removal moves the item into its own volume's Trash (your home Trash or `.Trash-<uid>`), or is refused. Windows: removal is refused on drives without a Recycle Bin, such as network shares and removable drives.
- Volume roots, the home folder, its standard folders and system folders cannot be selected. Hidden items need permission in Settings. Mounted volumes and unreadable folders cannot be selected.
- Right before acting, each item is compared with what you reviewed: the same file with the same size and date, or the same folder (on Windows, the same path). Changed or missing items are skipped and reported; scan again to act on them.
- A folder goes with everything it holds at that moment, including files added since the scan.
- On a name clash, the new item gets a numbered name. On FAT, exFAT and some network shares, macOS and Linux reserve the name first.
- A copy appears under its name only once complete. macOS and Linux build it in a private folder with restricted permissions; Windows uses a temporary name. **Stop** works within a file; a failed or stopped copy is cleaned up.
- On macOS and Linux, a folder swapped for a symbolic link while the app works is not followed. A copy does not cross into other volumes mounted inside the item.
- Move works within one volume. For another disk: copy, check the copy, then remove the original. The app never removes the source of a copy.
- Copies keep content, permissions and modification date, not extended attributes, resource forks, ACLs or NTFS alternate data streams.
- Another program can still change an item between the check and the operation. On Windows, and for the final step of a removal on macOS, items are addressed by path, so that window is wider.
- CSV reports and settings are written aside, then replace the old file whole.

The Windows and Linux builds have had less hands-on testing than the macOS build. See [known issues](KNOWN-ISSUES.md).

## Updates

The check asks `api.github.com` for the latest release. GitHub sees your IP address and the app version; nothing about your disks or files is sent. **Check for Updates…** is in the app menu on macOS, in the Help menu elsewhere.

When a newer version exists, you can download it, open its release page or skip it. The download goes to your Downloads folder and is checked against the release's SHA-256 checksums. The app never replaces itself: quit it and swap in the new version. On macOS the new copy may need Full Disk Access again.

## Linux and command line

The folder picker uses `xdg-desktop-portal`. The `.tar.gz` archive on the [releases page](https://github.com/tmanaud/sunburst-disk-analyser/releases) holds the bare binary; it also needs `libxkbcommon` and, on X11, `libxkbcommon-x11`.

```sh
sunburst-disk-analyser [FOLDER]       # scan a folder right away
sunburst-disk-analyser --lang fr      # en fr de es it ru uk zh
sunburst-disk-analyser --theme light  # force an appearance
sunburst-disk-analyser --demo         # sample data
```

Packaging: [macOS](../packaging/build-macos.sh), [Linux](../packaging/build-linux.sh), [Windows](../packaging/build-windows.ps1).
