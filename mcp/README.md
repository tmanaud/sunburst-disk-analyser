# Sunburst Disk Analyser, MCP server

`sunburst-mcp` lets an AI assistant (Claude Code, Claude Desktop, any MCP client) scan folders
and find where disk space goes, with the scanner of the desktop app. It speaks the Model
Context Protocol over stdio and is **read-only**: it never deletes, moves or writes a file.

## Tools

| Tool | What it answers |
|---|---|
| `disks` | Mounted filesystems with size, used and free space |
| `scan` | Scans a folder and keeps the result in memory; reused until `refresh: true` |
| `list` | Biggest items of a scanned folder, as a tree down to 4 levels, with share, file count and newest file date |
| `biggest_files` | Biggest files below a folder |
| `search` | Files by name, category (video, image, audio, document, archive), minimum size and age |
| `by_type` | Space by category and by extension |
| `stale_folders` | Biggest folders whose newest file is older than N days: candidates for archiving |
| `duplicates` | Probable copies: folders with the same size, file count and names inside, then files with the same name and size. Contents are not compared |

The scan is the app's own (`src/scan.rs`, `src/tree.rs`, `src/pinned.rs` and `src/filter.rs`,
compiled in unchanged): parallel, sizes as allocated on disk, hard links counted once, symbolic
links not followed, and it stays on the scanned folder's filesystem. A finished scan of 3.7
million files (1.5 TB) takes about 20 seconds on an NVMe disk; the other tools answer in
under 2 seconds from memory.

## Refusing some scans

`--deny <path>` refuses any scan that would cover `<path>` whole: the path itself or one of
its parents. Its subfolders can still be scanned. For example, `--deny / --deny ~` makes the
assistant scan one project folder at a time rather than the whole disk or home folder.

## Build

```sh
cd mcp
cargo build --release      # target/release/sunburst-mcp
cargo test --release
```

The crate only depends on `parking_lot`, `serde_json`, `sysinfo` and `libc`, at the versions of
the app's `Cargo.lock`. For a binary that runs on any Linux, build for musl (the official
`rust:alpine` image does it natively).

## Register

Claude Code:

```sh
claude mcp add -s user local-disk-sunburst -- /path/to/sunburst-mcp --deny / --deny ~
```

Other clients take the same command and arguments in their MCP configuration.
