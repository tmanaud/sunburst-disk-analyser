//! Arena-backed file tree produced by the scanner.
//!
//! Every directory's entries are appended in one batch, so the children of a
//! node always form a contiguous range in the arena.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

pub type NodeId = u32;
pub const NO_NODE: NodeId = u32::MAX;
/// Most items one scan keeps, which bounds its memory to a few gigabytes
/// whatever the scanned tree looks like.
pub const MAX_NODES: usize = 40_000_000;

pub const F_DIR: u8 = 1;
pub const F_HIDDEN: u8 = 1 << 1;
pub const F_SYMLINK: u8 = 1 << 2;
/// Directory that could not be read (permission denied, I/O error).
pub const F_DENIED: u8 = 1 << 3;
/// Removed or moved away after the scan.
pub const F_DELETED: u8 = 1 << 4;
/// Directory presented as a single file (macOS bundles such as `.app`).
pub const F_PACKAGE: u8 = 1 << 5;
/// Directory on another filesystem that the scan did not descend into.
pub const F_OTHER_FS: u8 = 1 << 6;

pub struct Node {
    pub name: Box<OsStr>,
    /// Bytes on disk for files, sum of descendants for directories.
    pub size: u64,
    /// Modification time, seconds since the Unix epoch (0 = unknown).
    pub mtime: i64,
    pub parent: NodeId,
    pub child_start: u32,
    pub child_count: u32,
    /// Number of files in the subtree (1 for a file).
    pub items: u32,
    pub flags: u8,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.flags & F_DIR != 0
    }
    pub fn is_hidden(&self) -> bool {
        self.flags & F_HIDDEN != 0
    }
    pub fn is_deleted(&self) -> bool {
        self.flags & F_DELETED != 0
    }
    pub fn is_package(&self) -> bool {
        self.flags & F_PACKAGE != 0
    }
    pub fn display_name(&self) -> String {
        self.name.to_string_lossy().into_owned()
    }
}

/// One directory entry as reported by the scanner.
pub struct Entry {
    pub name: OsString,
    pub size: u64,
    pub mtime: i64,
    pub flags: u8,
}

pub struct Tree {
    pub nodes: Vec<Node>,
    /// Bumped on every structural or size change; used to invalidate caches.
    pub generation: u64,
    /// Folders that could not be listed, plus entries that could not be read.
    pub denied_dirs: u32,
    /// (device, inode) each folder had when it was scanned, where the system
    /// has them: tells a folder from another one put in its place since.
    pub dir_ids: std::collections::HashMap<NodeId, (u64, u64)>,
}

impl Tree {
    /// Creates a tree whose root node carries the full root path as its name.
    pub fn new(root: PathBuf) -> Self {
        let root_node = Node {
            name: root.into_os_string().into_boxed_os_str(),
            size: 0,
            mtime: 0,
            parent: NO_NODE,
            child_start: 0,
            child_count: 0,
            items: 0,
            flags: F_DIR,
        };
        Tree { nodes: vec![root_node], generation: 1, denied_dirs: 0, dir_ids: Default::default() }
    }

    pub fn root(&self) -> NodeId {
        0
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Appends the entries of `parent` and propagates sizes to all ancestors.
    /// Returns the id of the first inserted child.
    pub fn add_children(&mut self, parent: NodeId, mut entries: Vec<Entry>) -> NodeId {
        // Past the limit the rest of the folder is reported as unread.
        let room = MAX_NODES.saturating_sub(self.nodes.len());
        if entries.len() > room {
            entries.truncate(room);
            self.denied_dirs = self.denied_dirs.saturating_add(1);
            self.nodes[parent as usize].flags |= F_DENIED;
        }
        let start = self.nodes.len() as u32;
        let mut size = 0u64;
        let mut items = 0u32;
        self.nodes.reserve(entries.len());
        for e in entries {
            let is_dir = e.flags & F_DIR != 0;
            if !is_dir {
                size += e.size;
                items += 1;
            }
            self.nodes.push(Node {
                name: e.name.into_boxed_os_str(),
                size: if is_dir { 0 } else { e.size },
                mtime: e.mtime,
                parent,
                child_start: 0,
                child_count: 0,
                items: if is_dir { 0 } else { 1 },
                flags: e.flags,
            });
        }
        let count = self.nodes.len() as u32 - start;
        {
            let p = &mut self.nodes[parent as usize];
            p.child_start = start;
            p.child_count = count;
        }
        let mut cur = parent;
        while cur != NO_NODE {
            let n = &mut self.nodes[cur as usize];
            n.size = n.size.saturating_add(size);
            n.items = n.items.saturating_add(items);
            cur = n.parent;
        }
        self.generation += 1;
        start
    }

    /// A folder whose content the scan does not know: another volume, or a
    /// folder that could not be read completely.
    pub fn unscanned(&self, id: NodeId) -> bool {
        self.nodes[id as usize].flags & (F_OTHER_FS | F_DENIED) != 0
    }

    pub fn mark_denied(&mut self, id: NodeId) {
        self.nodes[id as usize].flags |= F_DENIED;
        self.denied_dirs = self.denied_dirs.saturating_add(1);
        self.generation += 1;
    }

    /// Live (not deleted) children of `id`.
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let n = &self.nodes[id as usize];
        (n.child_start..n.child_start + n.child_count)
            .filter(move |&c| self.nodes[c as usize].flags & F_DELETED == 0)
    }

    /// Children sorted by size, largest first (ties broken by name).
    pub fn sorted_children(&self, id: NodeId) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self.children(id).collect();
        v.sort_by(|&a, &b| {
            let (na, nb) = (&self.nodes[a as usize], &self.nodes[b as usize]);
            nb.size.cmp(&na.size).then_with(|| na.name.cmp(&nb.name))
        });
        v
    }

    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut parts: Vec<&OsStr> = Vec::new();
        let mut cur = id;
        while cur != NO_NODE {
            let n = &self.nodes[cur as usize];
            parts.push(&n.name);
            cur = n.parent;
        }
        let mut p = PathBuf::new();
        for part in parts.iter().rev() {
            p.push(part);
        }
        p
    }

    /// Ancestors from the root down to `id` (inclusive).
    pub fn chain(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = Vec::new();
        let mut cur = id;
        while cur != NO_NODE {
            v.push(cur);
            cur = self.nodes[cur as usize].parent;
        }
        v.reverse();
        v
    }

    pub fn is_ancestor(&self, ancestor: NodeId, mut id: NodeId) -> bool {
        while id != NO_NODE {
            if id == ancestor {
                return true;
            }
            id = self.nodes[id as usize].parent;
        }
        false
    }

    /// True when the node or one of its ancestors below the root is hidden.
    pub fn in_hidden(&self, id: NodeId) -> bool {
        let mut cur = id;
        while cur != NO_NODE && cur != self.root() {
            let n = &self.nodes[cur as usize];
            if n.is_hidden() {
                return true;
            }
            cur = n.parent;
        }
        false
    }

    /// Marks a node as gone and subtracts its size from every ancestor.
    pub fn remove(&mut self, id: NodeId) {
        let (size, items, parent) = {
            let n = &mut self.nodes[id as usize];
            if n.flags & F_DELETED != 0 {
                return;
            }
            n.flags |= F_DELETED;
            (n.size, n.items, n.parent)
        };
        let mut cur = parent;
        while cur != NO_NODE {
            let n = &mut self.nodes[cur as usize];
            n.size = n.size.saturating_sub(size);
            n.items = n.items.saturating_sub(items);
            cur = n.parent;
        }
        self.generation += 1;
    }

    /// The `limit` largest files (packages count as files), largest first.
    pub fn biggest_files(&self, limit: usize) -> Vec<NodeId> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let mut heap: BinaryHeap<Reverse<(u64, NodeId)>> = BinaryHeap::with_capacity(limit + 1);
        let mut stack = vec![self.root()];
        while let Some(dir) = stack.pop() {
            for c in self.children(dir) {
                let n = &self.nodes[c as usize];
                if n.is_dir() && !n.is_package() {
                    stack.push(c);
                    continue;
                }
                if heap.len() < limit {
                    heap.push(Reverse((n.size, c)));
                } else if let Some(Reverse((min, _))) = heap.peek() {
                    if n.size > *min {
                        heap.pop();
                        heap.push(Reverse((n.size, c)));
                    }
                }
            }
        }
        let mut v: Vec<(u64, NodeId)> = heap.into_iter().map(|r| r.0).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// Case-insensitive name search; returns the `limit` largest matches.
    /// `superseded` is polled regularly so an outdated search can stop early.
    pub fn search(&self, query: &str, limit: usize, superseded: &dyn Fn() -> bool) -> Vec<NodeId> {
        let q = query.to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<NodeId> = Vec::new();
        for (i, n) in self.nodes.iter().enumerate().skip(1) {
            if i % 8192 == 0 && superseded() {
                return Vec::new();
            }
            if n.flags & F_DELETED != 0 {
                continue;
            }
            if n.name.to_string_lossy().to_lowercase().contains(&q) {
                hits.push(i as NodeId);
            }
        }
        // A match inside a deleted folder is gone too.
        hits.retain(|&id| !self.has_deleted_ancestor(id));
        hits.sort_by(|&a, &b| self.nodes[b as usize].size.cmp(&self.nodes[a as usize].size));
        hits.truncate(limit);
        hits
    }

    /// True when the node or one of its ancestors was removed.
    pub fn is_gone(&self, id: NodeId) -> bool {
        self.nodes[id as usize].is_deleted() || self.has_deleted_ancestor(id)
    }

    fn has_deleted_ancestor(&self, id: NodeId) -> bool {
        let mut cur = self.nodes[id as usize].parent;
        while cur != NO_NODE {
            let n = &self.nodes[cur as usize];
            if n.flags & F_DELETED != 0 {
                return true;
            }
            cur = n.parent;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, size: u64, flags: u8) -> Entry {
        Entry { name: name.into(), size, mtime: 0, flags }
    }

    fn sample() -> Tree {
        let mut t = Tree::new(PathBuf::from("/root"));
        let first = t.add_children(0, vec![e("a", 0, F_DIR), e("b.txt", 10, 0), e(".h", 5, F_HIDDEN)]);
        t.add_children(first, vec![e("big.bin", 100, 0), e("small", 1, 0)]);
        t
    }

    #[test]
    fn sizes_propagate() {
        let t = sample();
        assert_eq!(t.node(0).size, 116);
        assert_eq!(t.node(0).items, 4);
        assert_eq!(t.node(1).size, 101);
    }

    #[test]
    fn sorted_and_paths() {
        let t = sample();
        let c = t.sorted_children(0);
        assert_eq!(t.node(c[0]).display_name(), "a");
        assert_eq!(t.path(4), PathBuf::from("/root/a/big.bin"));
        assert_eq!(t.chain(4), vec![0, 1, 4]);
        assert!(t.is_ancestor(1, 4));
        assert!(!t.is_ancestor(2, 4));
    }

    #[test]
    fn remove_updates_ancestors() {
        let mut t = sample();
        t.remove(4);
        assert_eq!(t.node(0).size, 16);
        assert_eq!(t.node(1).size, 1);
        assert_eq!(t.node(0).items, 3);
        assert_eq!(t.children(1).count(), 1);
        t.remove(4);
        assert_eq!(t.node(0).size, 16);
    }

    #[test]
    fn biggest_and_search() {
        let t = sample();
        let b = t.biggest_files(2);
        assert_eq!(b, vec![4, 2]);
        let s = t.search("B", 10, &|| false);
        assert_eq!(s, vec![4, 2]);
        assert!(t.in_hidden(3));
        assert!(!t.in_hidden(4));
    }
}
