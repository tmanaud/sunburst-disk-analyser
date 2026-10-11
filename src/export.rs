//! CSV report of a scanned location.

use crate::tree::{NodeId, Tree};
use crate::util::{csv_field, fmt_date, fmt_size};
use std::io::{self, BufWriter, Write};
use std::path::Path;

/// Writes one line per file and folder below `root`, folders first by size.
pub fn write_csv(tree: &Tree, root: NodeId, out: &Path) -> io::Result<u64> {
    let mut w = BufWriter::new(std::fs::File::create(out)?);
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
}
