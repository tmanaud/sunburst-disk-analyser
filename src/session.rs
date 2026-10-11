//! One scanned location: navigation state, selection and the cached,
//! lock-free view of the tree that the UI paints every frame.

use crate::chart::{self, Layout, SegKind};
use crate::ops::{self, OpItem, OpResult};
use crate::platform::Space;
use crate::scan::ScanHandle;
use crate::tree::*;
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Most rows built per list; the rest is summarised in a "more" row.
const MAX_ROWS: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Outline,
    Biggest,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GroupKind {
    Small,
    Hidden,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    No,
    Yes,
    /// A parent folder is selected.
    Inherited,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub id: NodeId,
    pub name: String,
    pub size: u64,
    pub flags: u8,
    pub check: Check,
    /// True for the item itself or when it sits inside a hidden folder.
    pub hidden: bool,
    /// Parent folders, used by the flat lists (biggest files, search).
    pub crumbs: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Row(Row),
    Group { kind: GroupKind, size: u64, count: usize, open: bool },
    /// Row inside an expanded group.
    Member(Row),
    /// Number of rows left out of a very long list.
    More(usize),
    /// Free or unattributed space of a volume.
    Pseudo { kind: SegKind, size: u64 },
}

pub struct Column {
    pub dir: NodeId,
    /// The folder could not be read, or only partly.
    pub denied: bool,
    pub items: Vec<Item>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckError {
    ParentSelected,
    Hidden,
    System,
    Empty,
}

pub struct Session {
    /// Distinguishes this scan from an earlier or later one of the same location.
    pub id: u64,
    pub name: String,
    pub scan: ScanHandle,
    pub space: Option<Space>,
    /// Folder shown in the first column and at the centre of the chart.
    pub view_root: NodeId,
    /// Chain of clicked items below `view_root`, one per column.
    pub open: Vec<NodeId>,
    pub tab: Tab,
    pub checked: BTreeSet<NodeId>,
    pub expanded: HashSet<(NodeId, GroupKind)>,
    pub biggest_limit: usize,
    pub search_open: bool,
    pub query: String,
    pub search_rows: Vec<Row>,
    pub searching: bool,
    search_seq: Arc<AtomicU64>,
    search_rx: Option<mpsc::Receiver<(u64, Vec<NodeId>)>>,
    search_hits: Vec<NodeId>,
    pub hover: Option<NodeId>,
    pub hover_from_chart: bool,
    /// Item clicked in a flat list (biggest files, search results).
    pub focus: Option<NodeId>,

    // Cached view, rebuilt by `refresh`.
    pub columns: Vec<Column>,
    pub layout: Layout,
    /// For each chart segment, whether it is part of the selection.
    pub seg_checked: Vec<bool>,
    pub biggest: Vec<Row>,
    pub biggest_more: bool,
    pub crumbs: Vec<(NodeId, String)>,
    pub total_size: u64,
    pub total_items: u32,
    pub denied: u32,
    pub view_name: String,
    pub checked_size: u64,
    pub was_done: bool,
    dirty: bool,
    seen_generation: u64,
    last_refresh: Instant,
}

impl Session {
    pub fn new(name: String, scan: ScanHandle, space: Option<Space>) -> Session {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Session {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            name,
            scan,
            space,
            view_root: 0,
            open: Vec::new(),
            tab: Tab::Outline,
            checked: BTreeSet::new(),
            expanded: HashSet::new(),
            biggest_limit: 50,
            search_open: false,
            query: String::new(),
            search_rows: Vec::new(),
            searching: false,
            search_seq: Arc::new(AtomicU64::new(0)),
            search_rx: None,
            search_hits: Vec::new(),
            hover: None,
            hover_from_chart: false,
            focus: None,
            columns: Vec::new(),
            layout: Layout::default(),
            seg_checked: Vec::new(),
            biggest: Vec::new(),
            biggest_more: false,
            crumbs: Vec::new(),
            total_size: 0,
            total_items: 0,
            denied: 0,
            view_name: String::new(),
            checked_size: 0,
            was_done: false,
            dirty: true,
            seen_generation: 0,
            last_refresh: Instant::now() - Duration::from_secs(10),
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// The item menu commands apply to: the last clicked one, else the folder shown.
    pub fn target(&self) -> NodeId {
        if self.tab == Tab::Biggest || self.search_open {
            if let Some(id) = self.focus {
                return id;
            }
        }
        self.open.last().copied().unwrap_or(self.view_root)
    }

    pub fn path_of(&self, id: NodeId) -> PathBuf {
        self.scan.tree.read().path(id)
    }

    // ---- navigation -------------------------------------------------------

    /// A row of column `col` was clicked.
    pub fn select(&mut self, col: usize, id: NodeId) {
        self.open.truncate(col);
        self.open.push(id);
        self.dirty = true;
    }

    /// Opens the columns leading to `id` (used by the chart and the flat lists).
    pub fn select_path(&mut self, id: NodeId) {
        let tree = self.scan.tree.read();
        if !tree.is_ancestor(self.view_root, id) {
            self.view_root = tree.root();
        }
        let chain = tree.chain(id);
        let from = chain.iter().position(|&c| c == self.view_root).map(|i| i + 1).unwrap_or(1);
        self.open = chain[from..].to_vec();
        // Make sure the item is visible when it sits in a collapsed group.
        for &c in &self.open {
            let n = tree.node(c);
            if n.is_hidden() {
                self.expanded.insert((n.parent, GroupKind::Hidden));
            } else if is_small(n.size, tree.node(n.parent).size) {
                self.expanded.insert((n.parent, GroupKind::Small));
            }
        }
        drop(tree);
        self.tab = Tab::Outline;
        self.search_open = false;
        self.dirty = true;
    }

    /// Makes `id` the folder shown in the first column and the chart centre.
    pub fn drill(&mut self, id: NodeId) {
        if self.scan.tree.read().node(id).is_dir() {
            self.view_root = id;
            self.open.clear();
            self.tab = Tab::Outline;
            self.dirty = true;
        }
    }

    pub fn go_parent(&mut self) {
        let parent = self.scan.tree.read().node(self.view_root).parent;
        if parent != NO_NODE {
            let child = self.view_root;
            self.view_root = parent;
            self.open = vec![child];
            self.dirty = true;
        }
    }

    pub fn go_root(&mut self) {
        self.view_root = 0;
        self.open.clear();
        self.dirty = true;
    }

    /// A breadcrumb was clicked.
    pub fn go_crumb(&mut self, id: NodeId) {
        if let Some(i) = self.open.iter().position(|&c| c == id) {
            self.open.truncate(i + 1);
        } else {
            self.view_root = id;
            self.open.clear();
        }
        self.dirty = true;
    }

    pub fn toggle_group(&mut self, dir: NodeId, kind: GroupKind) {
        if !self.expanded.remove(&(dir, kind)) {
            self.expanded.insert((dir, kind));
        }
        self.dirty = true;
    }

    // ---- selection --------------------------------------------------------

    pub fn toggle_check(&mut self, id: NodeId, allow_hidden: bool) -> Result<(), CheckError> {
        if self.checked.remove(&id) {
            self.dirty = true;
            return Ok(());
        }
        let tree = self.scan.tree.read();
        let n = tree.node(id);
        if tree.chain(id).iter().any(|a| *a != id && self.checked.contains(a)) {
            return Err(CheckError::ParentSelected);
        }
        if !allow_hidden && tree.in_hidden(id) {
            return Err(CheckError::Hidden);
        }
        if id == tree.root() || ops::protected_reason(&tree.path(id)).is_some() {
            return Err(CheckError::System);
        }
        if n.size == 0 && !n.is_dir() {
            return Err(CheckError::Empty);
        }
        // A selected folder already covers everything inside it.
        let covered: Vec<NodeId> = self.checked.iter().copied().filter(|&c| tree.is_ancestor(id, c)).collect();
        drop(tree);
        for c in covered {
            self.checked.remove(&c);
        }
        self.checked.insert(id);
        self.dirty = true;
        Ok(())
    }

    pub fn deselect_all(&mut self) {
        self.checked.clear();
        self.dirty = true;
    }

    pub fn op_items(&self, only: Option<NodeId>) -> Vec<OpItem> {
        let tree = self.scan.tree.read();
        let ids: Vec<NodeId> = match only {
            Some(id) => vec![id],
            None => self.checked.iter().copied().collect(),
        };
        let mut items: Vec<OpItem> = ids
            .into_iter()
            .filter(|&id| !tree.is_gone(id))
            .map(|id| {
                let n = tree.node(id);
                let path = tree.path(id);
                OpItem { id, ident: ops::Ident::of(&path), path, size: n.size, is_dir: n.is_dir() }
            })
            .collect();
        items.sort_by(|a, b| b.size.cmp(&a.size));
        items
    }

    /// Updates the tree after a remove or move; copies leave it untouched.
    pub fn apply_results(&mut self, results: &[OpResult], removed: bool) {
        if removed {
            let mut tree = self.scan.tree.write();
            for r in results.iter().filter(|r| r.error.is_none()) {
                tree.remove(r.item.id);
            }
        }
        for r in results.iter().filter(|r| r.error.is_none()) {
            self.checked.remove(&r.item.id);
        }
        let tree = self.scan.tree.read();
        self.open.retain(|&id| !tree.is_gone(id));
        self.checked.retain(|&id| !tree.is_gone(id));
        if tree.chain(self.view_root).iter().any(|&a| tree.node(a).is_deleted()) {
            self.view_root = 0;
            self.open.clear();
        }
        drop(tree);
        self.dirty = true;
    }

    // ---- search -----------------------------------------------------------

    pub fn set_query(&mut self, query: String) {
        if query == self.query {
            return;
        }
        self.query = query;
        self.run_search();
    }

    /// Starts the search for the current query, superseding any running one.
    fn run_search(&mut self) {
        let query = self.query.clone();
        let seq = self.search_seq.fetch_add(1, Ordering::AcqRel) + 1;
        if query.trim().is_empty() {
            self.search_hits.clear();
            self.search_rows.clear();
            self.searching = false;
            self.search_rx = None;
            return;
        }
        let (tx, rx) = mpsc::channel();
        let tree = self.scan.tree.clone();
        let latest = self.search_seq.clone();
        self.search_rx = Some(rx);
        self.searching = true;
        std::thread::spawn(move || {
            let hits = tree.read().search(query.trim(), 300, &|| latest.load(Ordering::Acquire) != seq);
            let _ = tx.send((seq, hits));
        });
    }

    // ---- cached view ------------------------------------------------------

    /// Rebuilds the cached view when the tree or the navigation changed.
    /// Returns true while more refreshes are expected soon.
    pub fn refresh(&mut self) -> bool {
        if let Some(rx) = &self.search_rx {
            if let Ok((seq, hits)) = rx.try_recv() {
                if seq == self.search_seq.load(Ordering::Acquire) {
                    self.search_hits = hits;
                    self.searching = false;
                    self.search_rx = None;
                    self.dirty = true;
                }
            }
        }
        let done = self.scan.is_done();
        let just_finished = done && !self.was_done;
        if just_finished && !self.query.trim().is_empty() {
            // Results found while scanning miss whatever was discovered later.
            self.run_search();
        }
        let generation = self.scan.tree.read().generation;
        let changed = generation != self.seen_generation;
        let due = self.last_refresh.elapsed() >= Duration::from_millis(if self.tab == Tab::Biggest { 900 } else { 300 });
        if self.dirty || just_finished || (changed && due) {
            self.rebuild(done);
            self.seen_generation = generation;
            self.last_refresh = Instant::now();
            self.dirty = false;
        }
        self.was_done = done;
        !done || self.searching
    }

    fn rebuild(&mut self, done: bool) {
        let tree = self.scan.tree.read();
        if tree.node(self.view_root).is_deleted() {
            self.view_root = tree.root();
            self.open.clear();
        }
        let root = tree.node(tree.root());
        self.total_size = root.size;
        self.total_items = root.items;
        self.denied = tree.denied_dirs;
        self.view_name = if self.view_root == tree.root() { self.name.clone() } else { tree.node(self.view_root).display_name() };

        // Free and unattributed space only make sense for a whole volume.
        let (free, other) = match self.space {
            Some(s) if done && self.view_root == tree.root() => {
                (s.free, s.total.saturating_sub(s.free).saturating_sub(root.size))
            }
            _ => (0, 0),
        };

        let checked = &self.checked;
        let check_of = |id: NodeId| -> Check {
            if checked.is_empty() {
                Check::No
            } else if checked.contains(&id) {
                Check::Yes
            } else if tree.chain(id).iter().any(|a| checked.contains(a)) {
                Check::Inherited
            } else {
                Check::No
            }
        };
        let row = |id: NodeId, crumbs: bool| -> Row {
            let n = tree.node(id);
            Row {
                id,
                name: n.display_name(),
                size: n.size,
                flags: n.flags,
                check: check_of(id),
                hidden: tree.in_hidden(id),
                crumbs: if crumbs {
                    let chain = tree.chain(n.parent);
                    chain
                        .iter()
                        .enumerate()
                        .map(|(i, &a)| if i == 0 { self.name.clone() } else { tree.node(a).display_name() })
                        .collect()
                } else {
                    Vec::new()
                },
            }
        };

        // Columns: the view root, then every opened folder.
        let mut dirs = vec![self.view_root];
        dirs.extend(self.open.iter().copied().filter(|&id| tree.node(id).is_dir()));
        self.columns.clear();
        for (ci, dir) in dirs.into_iter().enumerate() {
            let psize = tree.node(dir).size;
            let (mut normal, mut small, mut hidden) = (Vec::new(), Vec::new(), Vec::new());
            let (mut small_size, mut hidden_size) = (0u64, 0u64);
            for c in tree.sorted_children(dir) {
                let n = tree.node(c);
                if n.is_hidden() {
                    hidden_size += n.size;
                    hidden.push(c);
                } else if is_small(n.size, psize) {
                    small_size += n.size;
                    small.push(c);
                } else {
                    normal.push(c);
                }
            }
            let mut items: Vec<Item> = Vec::new();
            let extra = normal.len().saturating_sub(MAX_ROWS);
            // A clicked item stays listed even when it is beyond the cut.
            let pinned: Vec<NodeId> = normal.iter().skip(MAX_ROWS).copied().filter(|c| self.open.contains(c)).collect();
            items.extend(normal.into_iter().take(MAX_ROWS).chain(pinned).map(|c| Item::Row(row(c, false))));
            if extra > 0 {
                items.push(Item::More(extra));
            }
            for (kind, list, size) in [(GroupKind::Small, small, small_size), (GroupKind::Hidden, hidden, hidden_size)] {
                if list.is_empty() {
                    continue;
                }
                let open = self.expanded.contains(&(dir, kind));
                items.push(Item::Group { kind, size, count: list.len(), open });
                if open {
                    let extra = list.len().saturating_sub(MAX_ROWS);
                    let pinned: Vec<NodeId> = list.iter().skip(MAX_ROWS).copied().filter(|c| self.open.contains(c)).collect();
                    items.extend(list.into_iter().take(MAX_ROWS).chain(pinned).map(|c| Item::Member(row(c, false))));
                    if extra > 0 {
                        items.push(Item::More(extra));
                    }
                }
            }
            if ci == 0 {
                if other > 0 {
                    items.push(Item::Pseudo { kind: SegKind::Other, size: other });
                }
                if free > 0 {
                    items.push(Item::Pseudo { kind: SegKind::Free, size: free });
                }
            }
            self.columns.push(Column { dir, denied: tree.node(dir).flags & F_DENIED != 0, items });
        }

        self.layout = chart::build(&tree, self.view_root, free, other);
        self.seg_checked = self
            .layout
            .segs
            .iter()
            .map(|s| matches!(s.kind, SegKind::Node | SegKind::Small) && check_of(s.id) != Check::No)
            .collect();
        if self.focus.map(|id| tree.is_gone(id)).unwrap_or(false) {
            self.focus = None;
        }

        if self.tab == Tab::Biggest {
            let ids = tree.biggest_files(self.biggest_limit + 1);
            self.biggest_more = ids.len() > self.biggest_limit;
            self.biggest = ids.into_iter().take(self.biggest_limit).map(|id| row(id, true)).collect();
        }
        self.search_rows = self
            .search_hits
            .iter()
            .filter(|&&id| !tree.is_gone(id))
            .map(|&id| row(id, true))
            .collect();

        // Breadcrumbs: from the scan root to the last opened item.
        self.crumbs.clear();
        let mut chain = tree.chain(self.view_root);
        chain.extend(self.open.iter().copied());
        for (i, id) in chain.into_iter().enumerate() {
            let name = if i == 0 { self.name.clone() } else { tree.node(id).display_name() };
            self.crumbs.push((id, name));
        }

        self.checked.retain(|&id| !tree.is_gone(id));
        self.checked_size = self.checked.iter().map(|&id| tree.node(id).size).sum();
    }
}

/// Items too small to matter next to their siblings are grouped.
pub fn is_small(size: u64, parent_size: u64) -> bool {
    size == 0 || (size as u128) * 1_000_000 < parent_size as u128
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn e(name: &str, size: u64, flags: u8) -> Entry {
        Entry { name: name.into(), size, mtime: 0, flags }
    }

    fn session() -> Session {
        let mut t = Tree::new(PathBuf::from("/tmp/sunburst-test-root"));
        let first = t.add_children(
            0,
            vec![e("docs", 0, F_DIR), e("movie.mkv", 5_000_000_000, 0), e(".cache", 0, F_DIR | F_HIDDEN), e("note.txt", 100, 0)],
        );
        t.add_children(first, vec![e("a.pdf", 2_000_000, 0), e("b.pdf", 1_000_000, 0)]);
        t.add_children(first + 2, vec![e("blob", 900_000_000, 0)]);
        let mut s = Session::new("root".into(), ScanHandle::finished(t), None);
        s.refresh();
        s
    }

    fn names(c: &Column) -> Vec<String> {
        c.items
            .iter()
            .map(|i| match i {
                Item::Row(r) | Item::Member(r) => r.name.clone(),
                Item::Group { kind, .. } => format!("{kind:?}"),
                Item::More(n) => format!("more {n}"),
                Item::Pseudo { kind, .. } => format!("{kind:?}"),
            })
            .collect()
    }

    #[test]
    fn groups_small_and_hidden_items() {
        let mut s = session();
        assert_eq!(names(&s.columns[0]), ["movie.mkv", "docs", "Small", "Hidden"]);
        s.toggle_group(0, GroupKind::Hidden);
        s.refresh();
        assert_eq!(names(&s.columns[0]), ["movie.mkv", "docs", "Small", "Hidden", ".cache"]);
    }

    #[test]
    fn navigation_builds_columns_and_crumbs() {
        let mut s = session();
        s.select(0, 1);
        s.refresh();
        assert_eq!(s.columns.len(), 2);
        assert_eq!(names(&s.columns[1]), ["a.pdf", "b.pdf"]);
        assert_eq!(s.crumbs.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["root", "docs"]);
        s.drill(1);
        s.refresh();
        assert_eq!(s.columns.len(), 1);
        assert_eq!(s.view_name, "docs");
        s.go_parent();
        s.refresh();
        assert_eq!(s.view_root, 0);
        assert_eq!(s.open, vec![1]);
        s.select_path(6);
        s.refresh();
        assert_eq!(s.open, vec![1, 6]);
    }

    #[test]
    fn selection_rules() {
        let mut s = session();
        assert_eq!(s.toggle_check(6, false), Ok(()));
        assert_eq!(s.toggle_check(1, false), Ok(()));
        assert_eq!(s.checked.iter().copied().collect::<Vec<_>>(), vec![1], "children are covered by the folder");
        assert_eq!(s.toggle_check(5, false), Err(CheckError::ParentSelected));
        assert_eq!(s.toggle_check(7, false), Err(CheckError::Hidden));
        assert_eq!(s.toggle_check(7, true), Ok(()));
        assert_eq!(s.toggle_check(0, true), Err(CheckError::System));
        s.refresh();
        assert_eq!(s.checked_size, 903_000_000);
        s.select(0, 1);
        s.refresh();
        match &s.columns[1].items[0] {
            Item::Row(r) => assert_eq!(r.check, Check::Inherited),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn results_update_the_tree() {
        let mut s = session();
        s.toggle_check(2, false).unwrap();
        let items = s.op_items(None);
        let results: Vec<OpResult> = items.into_iter().map(|item| OpResult { item, error: None, renamed: None }).collect();
        s.apply_results(&results, true);
        s.refresh();
        assert_eq!(s.total_size, 903_000_100);
        assert!(s.checked.is_empty());
        assert_eq!(names(&s.columns[0]), ["docs", "Small", "Hidden"]);
    }
}
