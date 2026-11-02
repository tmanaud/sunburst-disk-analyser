# Known issues and open work

What is still unverified, limited or not done. Read it before touching `src/ops.rs`, `src/pinned.rs` or `src/scan.rs`. What users are promised is in [DETAILS.md](DETAILS.md).

## Not verified

1. **Windows, beyond the unit tests.** The app has never been launched on Windows.
   - `Ident::of` reads the volume serial and file index with `GetFileInformationByHandle` on a handle opened with no access rights. It works on the CI's temporary files; untested on network drives, cloud placeholders and files the user cannot open. Where it fails, the operation is refused ("changed since it was scanned").
   - `has_recycle_bin` allows removal only on `DRIVE_FIXED` drives. Unchecked on external USB disks, mapped drives, `subst` drives and `\\?\` paths.
   - Whether the `trash` crate still deletes for good on a fixed drive (Recycle Bin turned off, item larger than the bin).
   - To do, in a VM: remove to the Recycle Bin; remove a file modified after the scan (refused); remove on a USB stick or a network drive (refused); copy and move onto an existing name (numbered name, nothing replaced); stop a large copy (nothing left).
2. **macOS volume without a Trash.** `NSFileManager` is assumed to refuse rather than delete. Not tested (network share, FAT/exFAT stick).
3. **Scan speed.** `list()` in `src/scan.rs` collects names, then calls `fstatat` per entry. The last benchmark (74 GB, 590,000 files, 2.5 s) predates this change: re-measure before quoting a figure.
4. **File systems without exclusive rename** (FAT, exFAT, SMB). The `rename_reserved` fallback is unit-tested by calling it directly, never on such a file system.
5. **Deep trees.** A copy holds two descriptors per folder level and raises the open-files limit to 8192. Beyond that it should fail with "too many open files" and clean up; not exercised.

## Known limits

- **Windows is path-based.** The handle-relative protection exists on Unix only. On Windows a folder swapped for a link during a scan or a copy can redirect it, a folder is matched to the scan by name and type only, and copies have no private staging folder. To decide: port handle-relative access (`NtCreateFile` with a root directory handle, or the `cap-std` crate).
- **macOS removal ends with a path.** The item is verified through a directory handle, then handed to the `trash` crate by path.
- **Folders are not snapshots.** A folder is acted on with whatever it contains at that moment; only its own identity is checked.
- **A file modified after the scan is refused** until its folder is scanned again.
- **Staging folder ownership** is only checked when the destination is writable by group or others, so that network mounts that remap owners keep working.
- **Permission errors on copy** (`EPERM`, `ENOTSUP`, `EINVAL` from `fchmod`) are ignored on purpose, for file systems without permissions.
- **Settings write errors are silent**: `Settings::save` cannot tell the user.
- **Cloud files.** `no_cloud_downloads` ignores the return value of `setiopolicy_np`: if that call fails, a scan could download cloud-only files.
- **Operation error messages are English only**: they are not in `src/strings.rs`. To do: translate them.
- **No dependency advisory scan.** To do: add `cargo audit` or `cargo deny` to the workflow.
- **Unix systems other than macOS and Linux** compile the Unix path but always use the reserved-name rename and cannot report folder read errors. Untested, not a release target.
- **Update downloads are checked, not authenticated.** The SHA-256 comes from `SHA256SUMS.txt` in the same release: it catches a damaged download, not a release replaced by someone with write access to the repository. The app only downloads from this repository's release URLs, over HTTPS, and never installs anything. Signed assets (for example with minisign, the public key built into the app) are needed before any self-update.
- **The update dialog has not met a real newer release yet**: 1.1.0 is the first version with it. The download was checked against the published v1.1.0 (`cargo test -- --ignored published_release_downloads`, macOS), the "newer version" path with a lowered version number, and the dialog through `--screenshot update`.
- **A cancelled update download can linger** until its time budget runs out (3 to 30 minutes, from the file size): a blocked network read cannot be interrupted. Once cancelled it never publishes anything, and each attempt has its own partial file.
- **Update publishing** uses a hard link, or else an exclusive rename (`renamex_np`, `renameat2`, `MoveFileExW` without replacement); where neither exists, the download fails rather than risk replacing a file. Not tried on FAT/exFAT.
- **Thumbnail programs** (ffmpeg, Quick Look) running over 6 s are stopped with their process group on Unix and their job object on Windows. The group is signalled before the program is reaped, so its id cannot have been reused.
  - Not stopped: a process that leaves its group (`setsid`), one started on Windows just before the job is assigned, and leftovers of a program that ended normally.
  - Without a job object, Windows makes no thumbnail. The Windows side was compiled, never run.
  - ffmpeg only gets self-contained video formats (`-format_whitelist`) and local files (`-protocol_whitelist file`).
- **Local builds embed the build machine's home path** in panic messages. Publish CI builds only, or add `--remap-path-prefix`.

## How it was verified

- macOS: unit tests, plus the ignored test that moves a file to the real Trash (`cargo test -- --ignored remove_goes_to_trash`).
- Linux: the whole suite in a `rust:1-bookworm` container with the CI's system libraries. `remove_never_copies_across_volumes` ran as an unprivileged user with two tmpfs mounts: `NO_TRASH_DIR` on a volume whose root that user cannot write to (removal refused, file intact), `VOLUME_TRASH_DIR` on one it can (the item lands in `.Trash-<uid>`).
- Windows: the unit tests on the CI runner (`windows-latest`): copy, move, collision names, the identity check and stopping a copy on an internal NTFS drive. No default test covers removal.
- The pinned release workflow built all three platforms and published v1.1.0 on 2026-11-02.
