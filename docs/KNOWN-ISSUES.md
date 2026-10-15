# Known issues and open work

State after the security pass of October 2026. Read this before touching
`src/ops.rs`, `src/pinned.rs` or `src/scan.rs`. User-facing guarantees are in
the Safety section of the README; this file is what is still unverified,
limited or not done.

## What the security pass changed

Two independent reviews listed the problems below; all were addressed unless
noted further down.

- Linux "Remove" could copy an item to the home trash and then delete the
  original for good. It is now a rename into a trash folder of the item's own
  volume (`xdg_trash` in `src/ops.rs`), or it is refused.
- Copy, move and scan re-opened paths after checking them. On Unix they now
  go through directory handles and never through a symbolic link
  (`src/pinned.rs`).
- The "changed since reviewed" check was weak. Identity is now volume + file
  number, plus length and date for files, compared against the scan and
  against the review; a missing identity is refused.
- Mounted volumes and unreadable folders inside a scan can no longer be
  selected.
- Copies are built in a private staging folder (0700), files created 0600.
- Stop works inside an item being copied; the copy walk is iterative.
- Exclusive rename has a safe fallback (name reserved first) on file systems
  without one.
- CSV export and the settings file are written aside and renamed into place.
- The scan keeps at most 40 million items, 4 million per folder.
- Windows "Remove" is refused on drives without a Recycle Bin.
- Linux "Open" reveals a `.desktop` file instead of running it.
- The release workflow pins actions by commit hash, verifies appimagetool by
  SHA-256 and gives write access to the release job only.

## How it was verified

- macOS: unit tests, plus the ignored test that moves a file to the real
  Trash (`cargo test -- --ignored remove_goes_to_trash`).
- Linux: the whole suite in a `rust:1-bookworm` container, with the system
  libraries the CI installs. `remove_never_copies_across_volumes` was run as
  an unprivileged user with two tmpfs mounts: `NO_TRASH_DIR` pointing into a
  volume whose root that user cannot write to (removal refused, file intact)
  and `VOLUME_TRASH_DIR` into one it can (item lands in `.Trash-<uid>`).
- Windows: the unit tests pass on the CI runner (`windows-latest`), which
  covers copy, move, collision names, the identity check and stopping a copy
  on an internal NTFS drive. The app itself was never launched there, and
  removal is not covered by a test that runs by default.
- The pinned release workflow ran green on all three platforms on its first
  push (build jobs; the release job only runs on a tag).

## Not verified

1. **Windows, beyond the unit tests.** The app was never launched. In
   particular:
   - `Ident::of` reads the volume serial and file index with
     `GetFileInformationByHandle` on a handle opened with no access rights.
     It works for the CI's temporary files; untested on network drives,
     cloud placeholders and files the user cannot open. Where it fails the
     operation is refused with "changed since it was scanned".
   - `has_recycle_bin` allows removal only when `GetDriveTypeW` says
     `DRIVE_FIXED`. Unchecked on real hardware: external USB hard disks,
     mapped drives, `subst` drives, paths with the `\\?\` prefix.
   - Whether the `trash` crate still deletes permanently in some case on a
     fixed drive (Recycle Bin disabled, item larger than the bin).
2. **macOS volume without a Trash.** Assumed that `NSFileManager` refuses
   rather than deletes. Not tested (network share, FAT/exFAT stick).
3. **Scan speed.** `list()` in `src/scan.rs` now collects names, then calls
   `fstatat` per entry. The README figure (74 GB, 590,000 files, 2.5 s) was
   measured before this change and has not been re-measured.
4. **File systems without exclusive rename** (FAT, exFAT, SMB). The
   `rename_reserved` fallback is unit-tested by calling it directly, never on
   such a file system.
5. **Deep trees.** A copy holds two descriptors per folder level and raises
   the open-files limit to 8192. Beyond that it fails with "too many open
   files" and cleans up; not exercised.
6. **The release job of the pinned workflow.** Only the build jobs have run
   since the pinning; the release job runs on the next tag.

## Known limits, by design or not done

- **Windows is still path-based.** The handle-relative protection exists on
  Unix only. On Windows a folder swapped for a link during a scan or a copy
  can still redirect it, a folder is matched to the scan by name and type
  only, and copies do not use a private staging folder.
- **macOS removal ends with a path.** The item is verified through a
  directory handle, then handed to the `trash` crate by path.
- **Folders are not content snapshots.** A folder is acted on with whatever
  it contains at that moment. Only the folder's own identity is checked.
- **A file modified after the scan is refused** until the folder is scanned
  again. Intended, but it is a behaviour change users will notice.
- **Staging folder ownership** is only checked when the destination is
  writable by group or others, to keep working on network mounts that remap
  owners.
- **Permission errors on copy** from file systems without permissions
  (`EPERM`, `ENOTSUP`, `EINVAL` from `fchmod`) are ignored on purpose.
- **Settings write errors are silent**: `Settings::save` has no way to tell
  the user.
- **Cloud files.** The return value of `setiopolicy_np` in
  `no_cloud_downloads` is ignored, so "never downloads cloud-only files" is
  not guaranteed if that call fails.
- **Operation error messages are English only**; they are not in
  `src/strings.rs`.
- **No dependency advisory scan** has been run (`cargo audit` is not part of
  the workflow).
- **Unix systems other than macOS and Linux** compile the Unix path but
  always use the reserved-name rename and cannot report directory read
  errors. Untested and not a release target.
- **Update downloads are checked, not authenticated.** The SHA-256 comes
  from `SHA256SUMS.txt` in the same release, so it catches a damaged
  download but not a release replaced by someone with write access to the
  repository. The app only downloads from this repository's release URLs,
  over HTTPS, and never installs anything itself. Signing the assets (for
  example with minisign, public key built into the app) would be needed
  before any in-place self-update.
- **The update dialog was only checked against the published v1.0.0**
  (`cargo test -- --ignored published_release_downloads`, macOS): the
  "newer version" path is exercised with a lowered version number, and the
  dialog through `--screenshot update`. It will meet a real newer release
  for the first time after the next tag.
- **Locally built binaries embed the build machine's home path** in panic
  messages. Publish CI builds only, or add `--remap-path-prefix`.

## Suggested next steps

1. Run the Windows build in a VM and go through: remove to the Recycle Bin;
   modify a file after the scan and try to remove it (refused); remove on a
   USB stick or network drive (refused); copy and move onto an existing name
   (numbered name, nothing replaced); stop a large copy (nothing left).
2. Re-measure scan speed on a large folder and update the README figure.
3. Decide whether to port handle-relative access to Windows
   (`NtCreateFile` with a root directory handle, or the `cap-std` crate).
4. Translate the operation error messages.
5. Add `cargo audit` (or `cargo deny`) to the workflow.
