//! CSV report of a scanned location.

use crate::tree::{NodeId, Tree};
use crate::util::{csv_field, fmt_date, fmt_size};
use std::io::{self, BufWriter, Write};
use std::path::Path;

/// Writes one line per file and folder below `root`, folders first by size.
///
/// The report is written to a new file next to `out` and renamed over it once
/// complete: a link sitting at that name is replaced, not written through,
/// and a failure leaves an existing report as it was.
pub fn write_csv(tree: &Tree, root: NodeId, out: &Path) -> io::Result<u64> {
    let mut name = std::ffi::OsString::from(".");
    name.push(out.file_name().ok_or(io::ErrorKind::InvalidInput)?);
    name.push(format!(".{}.tmp", std::process::id()));
    let temp = out.with_file_name(name);
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
    let written = write_rows(tree, root, file).and_then(|rows| std::fs::rename(&temp, out).map(|()| rows));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

fn write_rows(tree: &Tree, root: NodeId, file: std::fs::File) -> io::Result<u64> {
    let mut w = BufWriter::new(file);
    // Byte-order mark so spreadsheet applications detect UTF-8.
    w.write_all("\u{feff}".as_bytes())?;
    writeln!(w, "Path,Name,Kind,Size (bytes),Size,Files,Modified")?;
    let mut rows = 0u64;
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let n = tree.node(id);
        let kind = if n.is_package() {
            "Package"
        } else if n.is_dir() {
            "Folder"
        } else {
            "File"
        };
        writeln!(
            w,
            "{},{},{},{},{},{},{}",
            csv_field(&tree.path(id).to_string_lossy()),
            csv_field(&if id == tree.root() { crate::platform::display_name(&tree.path(id)) } else { n.display_name() }),
            kind,
            n.size,
            csv_field(&fmt_size(n.size)),
            n.items,
            csv_field(&fmt_date(n.mtime, false)),
        )?;
        rows += 1;
        if n.is_dir() {
            let mut kids = tree.sorted_children(id);
            kids.reverse();
            stack.extend(kids);
        }
    }
    w.flush()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Entry, F_DIR};
    use std::path::PathBuf;

    #[test]
    fn csv_lists_every_item() {
        let mut t = Tree::new(PathBuf::from("/data"));
        let first = t.add_children(
            0,
            vec![
                Entry { name: "dir, with comma".into(), size: 0, mtime: 0, flags: F_DIR },
                Entry { name: "a.txt".into(), size: 1500, mtime: 0, flags: 0 },
            ],
        );
        t.add_children(first, vec![Entry { name: "=evil".into(), size: 10, mtime: 0, flags: 0 }]);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("r.csv");
        assert_eq!(write_csv(&t, 0, &out).unwrap(), 4);
        let text = std::fs::read_to_string(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[2].contains("a.txt,File,1500,1.50 KB,1,"));
        assert!(lines[3].starts_with("\"/data/dir, with comma\"") || lines[3].contains("dir, with comma"));
        assert!(lines[4].contains("'=evil"));
    }

    #[cfg(unix)]
    #[test]
    fn csv_replaces_a_link_instead_of_writing_through_it() {
        let t = Tree::new(PathBuf::from("/data"));
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, b"untouched").unwrap();
        let out = dir.path().join("r.csv");
        std::os::unix::fs::symlink(&victim, &out).unwrap();
        write_csv(&t, 0, &out).unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
        assert!(!std::fs::symlink_metadata(&out).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "no temporary file is left");
    }
}
