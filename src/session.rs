//! One scanned location: navigation state, selection and the cached,
//! lock-free view of the tree that the UI paints every frame.

use crate::chart::{self, Layout, SegKind};
use crate::filter::{self, Filter};
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
/// Most steps kept for back and forward.
const MAX_HISTORY: usize = 200;
/// Most search results kept, the largest ones.
pub const SEARCH_LIMIT: usize = 300;

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

/// An arrow key pressed in the outline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
}

/// A list of rows that can be clicked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    /// An outline column, by index.
    Column(usize),
    /// The biggest files of several rows picked in a column.
    Merged,
    /// Biggest Files or the search results.
    Flat,
}

/// How a row was clicked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    /// Picks that row alone.
    One,
    /// Adds the row to the picked ones, or takes it out (Ctrl, ⌘ or Alt).
    Toggle,
    /// Picks every row from the anchor to this one (Shift).
    Range,
    /// Adds every row from the anchor to this one (Shift with Ctrl, ⌘ or Alt).
    AddRange,
}

/// Rows picked together in one list. The row clicked last is kept apart:
/// in `open` for a column, `focus` for the flat lists, `merged_focus` for
/// the merged pane.
#[derive(Clone, Default, Debug)]
pub struct Picks {
    /// Every picked row when there are several, empty otherwise.
    pub many: BTreeSet<NodeId>,
    /// Where a Shift-click range starts.
    anchor: Option<NodeId>,
}

impl Picks {
    /// Applies a click on `id`. `order` lists the rows as shown, `lead` is
    /// the row picked so far when there is only one. Returns the row now in
    /// the lead: the one clicked, or another picked one when it was taken out.
    fn click(&mut self, order: &[NodeId], lead: Option<NodeId>, id: NodeId, how: Click) -> Option<NodeId> {
        let pos = |r: NodeId| order.iter().position(|&o| o == r);
        let mut picked: BTreeSet<NodeId> = if self.many.is_empty() { lead.into_iter().collect() } else { std::mem::take(&mut self.many) };
        // A range starts at the anchor when it is in this list, else at the lead.
        let anchor = self.anchor.filter(|&a| pos(a).is_some()).or(lead.filter(|&l| pos(l).is_some()));
        let range = || -> BTreeSet<NodeId> {
            match (anchor.and_then(pos), pos(id)) {
                (Some(a), Some(b)) => order[a.min(b)..=a.max(b)].iter().copied().collect(),
                _ => BTreeSet::from([id]),
            }
        };
        let mut next = Some(id);
        match how {
            Click::One => picked = BTreeSet::from([id]),
            Click::Toggle => {
                if picked.remove(&id) {
                    next = lead.filter(|l| picked.contains(l)).or_else(|| order.iter().rev().find(|r| picked.contains(r)).copied());
                } else {
                    picked.insert(id);
                }
            }
            Click::Range => picked = range(),
            Click::AddRange => picked.extend(range()),
        }
        self.anchor = match how {
            Click::One | Click::Toggle => Some(id),
            Click::Range | Click::AddRange => anchor.or(Some(id)),
        };
        if picked.len() > 1 {
            self.many = picked;
        }
        next
    }

    /// Keeps only picks that are still listed; one left alone is no longer a group.
    fn keep(&mut self, listed: impl Fn(NodeId) -> bool) {
        self.many.retain(|&id| listed(id));
        if self.many.len() < 2 {
            self.many.clear();
        }
    }
}

/// Largest files found last, reused while the tree and the question stay the same.
#[derive(Default)]
struct Found {
    key: (u64, usize, Vec<NodeId>),
    ids: Vec<NodeId>,
}

impl Found {
    fn get(&mut self, tree: &Tree, items: Vec<NodeId>, limit: usize) -> &[NodeId] {
        let key = (tree.generation, limit, items);
        if self.key != key {
            self.ids = tree.biggest_files_in(&key.2, limit);
            self.key = key;
        }
        &self.ids
    }
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

impl Column {
    /// The items listed as rows, in order: groups and summaries are not rows.
    pub fn ids(&self) -> Vec<NodeId> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Row(r) | Item::Member(r) => Some(r.id),
                _ => None,
            })
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckError {
    ParentSelected,
    Hidden,
    System,
    Empty,
    /// Another volume or a folder that could not be read: its content is unknown.
    Unscanned,
}

pub struct Session {
    /// Distinguishes this scan from an earlier or later one of the same location.
    pub id: u64,
    pub name: String,
    pub scan: ScanHandle,
    pub space: Option<Space>,
    /// Chain of clicked items below the scan root, one per column.
    pub open: Vec<NodeId>,
    /// Folder at the centre of the chart: the one holding the last item of `open`.
    pub view_root: NodeId,
    /// Paths visited, for back and forward.
    history: Vec<Vec<NodeId>>,
    hist_pos: usize,
    /// The current step was made with the arrow keys.
    hist_keys: bool,
    /// Changes with every move, so the views can bring the selection into sight.
    pub nav_seq: u64,
    pub tab: Tab,
    pub checked: BTreeSet<NodeId>,
    pub expanded: HashSet<(NodeId, GroupKind)>,
    pub biggest_limit: usize,
    pub search_open: bool,
    pub query: String,
    pub filter: Filter,
    pub search_rows: Vec<Row>,
    pub searching: bool,
    /// The results stop at `SEARCH_LIMIT`: more items match.
    pub search_capped: bool,
    search_seq: Arc<AtomicU64>,
    search_rx: Option<mpsc::Receiver<(u64, Vec<NodeId>)>>,
    search_hits: Vec<NodeId>,
    pub hover: Option<NodeId>,
    pub hover_from_chart: bool,
    /// Item clicked in a flat list (biggest files, search results).
    pub focus: Option<NodeId>,
    /// Rows picked together in the column of the last item of `open`. With
    /// several, the next pane lists the biggest files they hold.
    pub col: Picks,
    pub flat: Picks,
    pub merged: Picks,
    /// Item clicked in the merged pane.
    pub merged_focus: Option<NodeId>,
    pub merged_limit: usize,
    /// Checkbox clicked last, where a Shift-click on another one starts.
    tick_anchor: Option<(Pane, NodeId)>,

    // Cached view, rebuilt by `refresh`.
    pub columns: Vec<Column>,
    pub layout: Layout,
    /// For each chart segment, whether it is part of the selection.
    pub seg_checked: Vec<bool>,
    pub biggest: Vec<Row>,
    pub biggest_more: bool,
    /// Biggest files of the rows picked in `col`, and their total size.
    pub merged_rows: Vec<Row>,
    pub merged_more: bool,
    pub picked_size: u64,
    biggest_found: Found,
    merged_found: Found,
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
    /// Estimated share of the scan that is done, 0 to 1. A volume is measured
    /// against its used space; the size of a folder is not known beforehand,
    /// so it goes by the share of its folder tree that has been read.
    pub fn progress(&self) -> f32 {
        if self.scan.is_done() {
            return 1.0;
        }
        // Only a finished scan is complete: a deep subtree can hold a share too small to show.
        let estimate = match self.space {
            Some(s) if s.total > s.free => (self.scan.bytes() as f64 / (s.total - s.free) as f64) as f32,
            _ => self.scan.progress(),
        };
        estimate.min(0.99)
    }

    pub fn new(name: String, scan: ScanHandle, space: Option<Space>) -> Session {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Session {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            name,
            scan,
            space,
            open: Vec::new(),
            view_root: 0,
            history: vec![Vec::new()],
            hist_pos: 0,
            hist_keys: false,
            nav_seq: 0,
            tab: Tab::Outline,
            checked: BTreeSet::new(),
            expanded: HashSet::new(),
            biggest_limit: 50,
            search_open: false,
            query: String::new(),
            filter: Filter::default(),
            search_rows: Vec::new(),
            searching: false,
            search_capped: false,
            search_seq: Arc::new(AtomicU64::new(0)),
            search_rx: None,
            search_hits: Vec::new(),
            hover: None,
            hover_from_chart: false,
            focus: None,
            col: Picks::default(),
            flat: Picks::default(),
            merged: Picks::default(),
            merged_focus: None,
            merged_limit: 50,
            tick_anchor: None,
            columns: Vec::new(),
            layout: Layout::default(),
            seg_checked: Vec::new(),
            biggest: Vec::new(),
            biggest_more: false,
            merged_rows: Vec::new(),
            merged_more: false,
            picked_size: 0,
            biggest_found: Found::default(),
            merged_found: Found::default(),
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
        if self.flat_shown() {
            if let Some(id) = self.focus {
                return id;
            }
        }
        if let Some(id) = self.merged_focus.filter(|_| self.many()) {
            return id;
        }
        self.open.last().copied().unwrap_or(self.view_root)
    }

    /// The file shown in the preview pane: the one at the end of the path, or
    /// the one clicked in the merged pane.
    pub fn selected_file(&self) -> Option<NodeId> {
        let id = if self.many() { self.merged_focus? } else { *self.open.last()? };
        let tree = self.scan.tree.read();
        (!tree.node(id).is_dir() && !tree.is_gone(id)).then_some(id)
    }

    /// Biggest Files or the search results are shown instead of the outline.
    pub fn flat_shown(&self) -> bool {
        self.tab == Tab::Biggest || self.search_open
    }

    /// Several rows are picked in an outline column: the merged pane follows it.
    pub fn many(&self) -> bool {
        !self.col.many.is_empty()
    }

    /// The rows of a list, as shown.
    fn order(&self, pane: Pane) -> Vec<NodeId> {
        let ids = |rows: &[Row]| rows.iter().map(|r| r.id).collect();
        match pane {
            Pane::Column(ci) => self.columns.get(ci).map(|c| c.ids()).unwrap_or_default(),
            Pane::Merged => ids(&self.merged_rows),
            Pane::Flat => ids(self.flat_rows()),
        }
    }

    pub fn flat_rows(&self) -> &[Row] {
        if self.search_open {
            &self.search_rows
        } else {
            &self.biggest
        }
    }

    /// Whether a row is picked, alone or with others.
    pub fn is_picked(&self, pane: Pane, id: NodeId) -> bool {
        match pane {
            Pane::Column(ci) => self.open.get(ci) == Some(&id) || (ci + 1 == self.open.len() && self.col.many.contains(&id)),
            Pane::Merged => self.merged_focus == Some(id) || self.merged.many.contains(&id),
            Pane::Flat => self.focus == Some(id) || self.flat.many.contains(&id),
        }
    }

    /// The rows picked together with `id`, when it is one of several.
    pub fn picked_with(&self, pane: Pane, id: NodeId) -> Option<&BTreeSet<NodeId>> {
        let picks = match pane {
            Pane::Column(ci) if ci + 1 == self.open.len() => &self.col,
            Pane::Column(_) => return None,
            Pane::Merged => &self.merged,
            Pane::Flat => &self.flat,
        };
        picks.many.contains(&id).then_some(&picks.many)
    }

    /// Whether the chart outlines `id`: it is picked in the list in sight.
    pub fn is_marked(&self, id: NodeId) -> bool {
        if self.flat_shown() {
            self.is_picked(Pane::Flat, id)
        } else if self.many() {
            self.col.many.contains(&id) || self.is_picked(Pane::Merged, id)
        } else {
            self.open.last() == Some(&id)
        }
    }

    /// Several rows are picked in the list in sight.
    pub fn has_picks(&self) -> bool {
        if self.flat_shown() {
            !self.flat.many.is_empty()
        } else {
            self.many()
        }
    }

    pub fn path_of(&self, id: NodeId) -> PathBuf {
        self.scan.tree.read().path(id)
    }

    // ---- navigation -------------------------------------------------------

    /// Moves to `path`, a chain of items below the scan root, its last item picked alone.
    fn go(&mut self, path: Vec<NodeId>, keys: bool) {
        if self.many() {
            self.dirty = true;
        }
        self.col = Picks { many: BTreeSet::new(), anchor: path.last().copied() };
        self.walk(path, keys);
    }

    /// Moves to `path`, leaving the picked rows as they are.
    fn walk(&mut self, path: Vec<NodeId>, keys: bool) {
        if path == self.open {
            return;
        }
        let tree = self.scan.tree.read();
        let same_folder = folder_of(&tree, &path) == folder_of(&tree, &self.open);
        drop(tree);
        self.view_root = centre_of(&path);
        self.open = path;
        if same_folder || (keys && self.hist_keys) {
            // Another item of the same folder, or one more key press: not a new step.
            self.history[self.hist_pos] = self.open.clone();
        } else {
            self.history.truncate(self.hist_pos + 1);
            if self.history.len() == MAX_HISTORY {
                self.history.remove(0);
            }
            self.history.push(self.open.clone());
            self.hist_pos = self.history.len() - 1;
            self.hist_keys = keys;
        }
        self.hist_keys &= keys;
        self.nav_seq += 1;
        self.dirty = true;
    }

    /// A row of column `col` was clicked.
    pub fn select(&mut self, col: usize, id: NodeId) {
        let mut path = self.open[..col.min(self.open.len())].to_vec();
        path.push(id);
        self.go(path, false);
    }

    /// Opens the columns leading to `id` (used by the chart and the flat lists).
    pub fn select_path(&mut self, id: NodeId) {
        let tree = self.scan.tree.read();
        let path = tree.chain(id)[1..].to_vec();
        // Make sure the item is visible when it sits in a collapsed group.
        for &c in &path {
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
        self.go(path, false);
    }

    /// One level up: the folder holding the selection becomes the selection.
    pub fn go_parent(&mut self) {
        let mut path = self.open.clone();
        if path.pop().is_some() {
            self.go(path, false);
        }
    }

    pub fn go_root(&mut self) {
        self.go(Vec::new(), false);
    }

    /// A breadcrumb was clicked.
    pub fn go_crumb(&mut self, id: NodeId) {
        let path = self.scan.tree.read().chain(id)[1..].to_vec();
        self.go(path, false);
    }

    pub fn can_go(&self, forward: bool) -> bool {
        if forward {
            self.hist_pos + 1 < self.history.len()
        } else {
            self.hist_pos > 0
        }
    }

    /// Back or forward through the visited folders. Returns false at either end.
    pub fn go_history(&mut self, forward: bool) -> bool {
        let tree = self.scan.tree.read();
        while self.can_go(forward) {
            self.hist_pos = if forward { self.hist_pos + 1 } else { self.hist_pos - 1 };
            let mut path = self.history[self.hist_pos].clone();
            // Items removed since the visit end the path early.
            if let Some(i) = path.iter().position(|&id| tree.is_gone(id)) {
                path.truncate(i);
            }
            if path != self.open {
                self.view_root = centre_of(&path);
                self.col = Picks { many: BTreeSet::new(), anchor: path.last().copied() };
                self.open = path;
                self.hist_keys = false;
                self.nav_seq += 1;
                self.dirty = true;
                return true;
            }
        }
        false
    }

    /// An arrow key: up and down move in the column of the selected item,
    /// left and right change column.
    pub fn step(&mut self, step: Step) {
        if self.many() {
            // The keys move from the row clicked last, alone again.
            self.col.many.clear();
            self.dirty = true;
        }
        if self.dirty {
            // The columns must match `open` before they are used to move in it.
            self.rebuild(self.scan.is_done());
        }
        let mut path = self.open.clone();
        let next = match (step, path.pop()) {
            (Step::Left, Some(_)) => None,
            (Step::Left, None) => return,
            // Into the folder just left on the path, or the first column when nothing is selected.
            (Step::Right, last) => {
                path.extend(last);
                self.columns.get(path.len()).and_then(|c| c.ids().first().copied())
            }
            (_, None) => self.columns.first().and_then(|c| c.ids().first().copied()),
            (_, Some(last)) => {
                let Some(col) = self.columns.get(path.len()) else { return };
                let ids = col.ids();
                let Some(i) = ids.iter().position(|&id| id == last) else { return };
                let j = if step == Step::Down { i + 1 } else { i.wrapping_sub(1) };
                ids.get(j).copied()
            }
        };
        match (step, next) {
            (Step::Left, _) => {}
            (_, Some(id)) => path.push(id),
            (_, None) => return,
        }
        self.go(path, true);
    }

    pub fn toggle_group(&mut self, dir: NodeId, kind: GroupKind) {
        if !self.expanded.remove(&(dir, kind)) {
            self.expanded.insert((dir, kind));
        }
        self.dirty = true;
    }

    // ---- picking several rows ---------------------------------------------

    /// A row of `pane` was clicked, alone or with Shift, Ctrl (⌘) or Alt.
    pub fn click(&mut self, pane: Pane, id: NodeId, how: Click) {
        if self.dirty {
            // A range is taken from the rows as shown.
            self.rebuild(self.scan.is_done());
        }
        let order = self.order(pane);
        match pane {
            Pane::Column(ci) => {
                // Rows picked in another column do not mix with this one.
                if ci + 1 != self.open.len() {
                    self.col.many.clear();
                }
                let before = self.col.many.clone();
                let mut picks = std::mem::take(&mut self.col);
                let next = picks.click(&order, self.open.get(ci).copied(), id, how);
                let mut path = self.open[..ci.min(self.open.len())].to_vec();
                path.extend(next);
                self.walk(path, false);
                if picks.many != before {
                    self.merged_limit = 50;
                }
                self.col = picks;
            }
            Pane::Merged => {
                let focus = self.merged.click(&order, self.merged_focus, id, how);
                if focus != self.merged_focus {
                    // The columns slide to the preview of the file clicked.
                    self.nav_seq += 1;
                }
                self.merged_focus = focus;
            }
            Pane::Flat => self.focus = self.flat.click(&order, self.focus, id, how),
        }
        self.dirty = true;
    }

    /// Shift with an arrow key: the rows picked in the column of the row
    /// clicked last run to the one above or below.
    pub fn extend(&mut self, step: Step) {
        if self.dirty {
            self.rebuild(self.scan.is_done());
        }
        let Some(&lead) = self.open.last() else { return self.step(step) };
        let ci = self.open.len() - 1;
        let order = self.order(Pane::Column(ci));
        let Some(i) = order.iter().position(|&id| id == lead) else { return };
        let j = match step {
            Step::Down => i + 1,
            Step::Up => i.wrapping_sub(1),
            _ => return,
        };
        if let Some(&next) = order.get(j) {
            self.click(Pane::Column(ci), next, Click::Range);
        }
    }

    /// Leaves the row clicked last alone in the list in sight, or in the
    /// merged pane first when it holds several. Returns false when nothing changed.
    pub fn unpick(&mut self) -> bool {
        let (picks, lead) = if self.flat_shown() {
            (&mut self.flat, self.focus)
        } else if !self.merged.many.is_empty() {
            (&mut self.merged, self.merged_focus)
        } else {
            (&mut self.col, self.open.last().copied())
        };
        let changed = !picks.many.is_empty();
        picks.many.clear();
        // A range starts afresh from the row left.
        picks.anchor = lead;
        self.dirty |= changed;
        changed
    }

    pub fn more_merged(&mut self) {
        self.merged_limit += 50;
        self.dirty = true;
    }

    // ---- selection --------------------------------------------------------

    pub fn toggle_check(&mut self, id: NodeId, allow_hidden: bool) -> Result<(), CheckError> {
        if self.checked.remove(&id) {
            self.dirty = true;
            return Ok(());
        }
        self.check(id, allow_hidden)
    }

    /// The checkbox of `id` in `pane` was clicked; with `range` (Shift), every
    /// row from the checkbox clicked before in the same list takes its new state.
    pub fn tick(&mut self, pane: Pane, id: NodeId, range: bool, allow_hidden: bool) -> Option<CheckError> {
        let from = self.tick_anchor.filter(|a| range && a.0 == pane).map(|a| a.1);
        self.tick_anchor = Some((pane, id));
        let order = self.order(pane);
        let pos = |r: NodeId| order.iter().position(|&o| o == r);
        match (from.and_then(pos), pos(id)) {
            (Some(a), Some(b)) if a != b => {
                let on = !self.checked.contains(&id);
                self.set_checks(&order[a.min(b)..=a.max(b)], on, allow_hidden)
            }
            _ => self.toggle_check(id, allow_hidden).err(),
        }
    }

    /// Ticks or unticks several items. Returns why some could not be ticked.
    pub fn set_checks(&mut self, ids: &[NodeId], on: bool, allow_hidden: bool) -> Option<CheckError> {
        self.dirty = true;
        if !on {
            for id in ids {
                self.checked.remove(id);
            }
            return None;
        }
        let (mut ticked, mut refused, mut covered) = (false, None, false);
        for &id in ids {
            match self.check(id, allow_hidden) {
                Ok(()) => ticked = true,
                // Inside a ticked folder: it goes with it already.
                Err(CheckError::ParentSelected) => covered = true,
                Err(e) => refused = refused.or(Some(e)),
            }
        }
        refused.or((covered && !ticked).then_some(CheckError::ParentSelected))
    }

    /// Ticks `id` unless the selection rules forbid it.
    fn check(&mut self, id: NodeId, allow_hidden: bool) -> Result<(), CheckError> {
        if self.checked.contains(&id) {
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
        if tree.unscanned(id) {
            return Err(CheckError::Unscanned);
        }
        if n.size == 0 && !n.is_dir() {
            return Err(CheckError::Empty);
        }
        // A selected folder already covers everything inside it.
        let covered: Vec<NodeId> = if n.is_dir() { self.checked.iter().copied().filter(|&c| tree.is_ancestor(id, c)).collect() } else { Vec::new() };
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

    /// What an operation acts on: the ticked items, or `only` these ones.
    pub fn op_items(&self, only: Option<&[NodeId]>) -> Vec<OpItem> {
        let tree = self.scan.tree.read();
        let ids: Vec<NodeId> = match only {
            Some(ids) => ids.to_vec(),
            None => self.checked.iter().copied().collect(),
        };
        let mut items: Vec<OpItem> = ids
            .into_iter()
            .filter(|&id| !tree.is_gone(id))
            .map(|id| {
                let n = tree.node(id);
                let path = tree.path(id);
                // An item that no longer matches the scan gets no identity and
                // is refused when the operation runs.
                let scanned_as = tree.dir_ids.get(&id).copied();
                let ident = ops::Ident::of(&path).filter(|_| ops::unchanged_since_scan(&path, n.is_dir(), n.mtime, scanned_as));
                OpItem { id, ident, path, size: n.size, is_dir: n.is_dir() }
            })
            .collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.size));
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
        self.checked.retain(|&id| !tree.is_gone(id));
        drop(tree);
        // Sizes changed: a folder may no longer match a size filter, and
        // capped results have room for more.
        if removed && self.has_search() {
            self.run_search();
        }
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

    pub fn set_filter(&mut self, filter: Filter) {
        if filter == self.filter {
            return;
        }
        self.filter = filter;
        self.run_search();
    }

    /// Something to search for: a name or a filter.
    pub fn has_search(&self) -> bool {
        !self.query.trim().is_empty() || !self.filter.is_empty()
    }

    /// Starts the search for the current query, superseding any running one.
    fn run_search(&mut self) {
        let query = self.query.clone();
        let filter = self.filter;
        let seq = self.search_seq.fetch_add(1, Ordering::AcqRel) + 1;
        if !self.has_search() {
            self.search_hits.clear();
            self.search_rows.clear();
            self.searching = false;
            self.search_capped = false;
            self.search_rx = None;
            return;
        }
        let (tx, rx) = mpsc::channel();
        let tree = self.scan.tree.clone();
        let latest = self.search_seq.clone();
        self.search_rx = Some(rx);
        self.searching = true;
        std::thread::spawn(move || {
            let hits = tree.read().search(query.trim(), &filter, filter::now(), SEARCH_LIMIT + 1, &|| latest.load(Ordering::Acquire) != seq);
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
                    self.search_capped = hits.len() > SEARCH_LIMIT;
                    self.search_hits = hits;
                    self.search_hits.truncate(SEARCH_LIMIT);
                    self.searching = false;
                    self.search_rx = None;
                    self.dirty = true;
                }
            }
        }
        let done = self.scan.is_done();
        let just_finished = done && !self.was_done;
        if just_finished && self.has_search() {
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
        // A removed item ends the path. When it was picked with other rows,
        // one of those that are left takes its place.
        self.col.many.retain(|&id| !tree.is_gone(id));
        if let Some(i) = self.open.iter().position(|&id| tree.is_gone(id)) {
            self.open.truncate(i);
            self.open.extend(self.col.many.iter().next_back());
            self.history[self.hist_pos] = self.open.clone();
            self.nav_seq += 1;
        }
        // Rows stay picked together only next to the row clicked last.
        let lead = self.open.last().map(|&l| (l, tree.node(l).parent));
        let siblings = lead.is_some_and(|(l, parent)| self.col.many.contains(&l) && self.col.many.iter().all(|&id| tree.node(id).parent == parent));
        if self.col.many.len() < 2 || !siblings {
            self.col.many.clear();
        }
        let many = !self.col.many.is_empty();
        self.view_root = centre_of(&self.open);
        let root = tree.node(tree.root());
        self.total_size = root.size;
        self.total_items = root.items;
        self.denied = tree.denied_dirs;
        self.view_name = if self.view_root == tree.root() { self.name.clone() } else { tree.node(self.view_root).display_name() };

        // Free and unattributed space only make sense for a whole volume.
        let (free, other) = match self.space {
            Some(s) if done => (s.free, s.total.saturating_sub(s.free).saturating_sub(root.size)),
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
        let row = |id: NodeId, crumbs: Vec<String>| -> Row {
            let n = tree.node(id);
            Row { id, name: n.display_name(), size: n.size, flags: n.flags, check: check_of(id), hidden: tree.in_hidden(id), crumbs }
        };
        let name_of = |id: NodeId| if id == tree.root() { self.name.clone() } else { tree.node(id).display_name() };
        // The folders holding an item, from the scan root down.
        let path_to = |id: NodeId| -> Vec<String> { tree.chain(tree.node(id).parent).into_iter().map(name_of).collect() };
        // The same below `base`, the folder all rows of the merged pane share;
        // just its name for an item right inside it.
        let path_below = |id: NodeId, base: NodeId| -> Vec<String> {
            let chain = tree.chain(tree.node(id).parent);
            let below: Vec<String> = chain.into_iter().skip_while(|&a| a != base).skip(1).map(name_of).collect();
            if below.is_empty() {
                vec![name_of(base)]
            } else {
                below
            }
        };

        // Columns: the scan root, then every opened folder, but not the content
        // of rows picked together: the merged pane comes in its place.
        let shown = if many { &self.open[..self.open.len() - 1] } else { &self.open[..] };
        let mut dirs = vec![tree.root()];
        dirs.extend(shown.iter().copied().filter(|&id| tree.node(id).is_dir()));
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
            items.extend(normal.into_iter().take(MAX_ROWS).chain(pinned).map(|c| Item::Row(row(c, Vec::new()))));
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
                    items.extend(list.into_iter().take(MAX_ROWS).chain(pinned).map(|c| Item::Member(row(c, Vec::new()))));
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

        self.layout = if self.view_root == tree.root() {
            chart::build(&tree, self.view_root, free, other)
        } else {
            chart::build(&tree, self.view_root, 0, 0)
        };
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
            let ids = self.biggest_found.get(&tree, vec![tree.root()], self.biggest_limit + 1);
            self.biggest_more = ids.len() > self.biggest_limit;
            self.biggest = ids.iter().take(self.biggest_limit).map(|&id| row(id, path_to(id))).collect();
        }
        self.search_rows = self
            .search_hits
            .iter()
            .filter(|&&id| !tree.is_gone(id))
            .map(|&id| row(id, path_to(id)))
            .collect();
        let flat_ids: HashSet<NodeId> = self.flat_rows().iter().map(|r| r.id).collect();
        self.flat.keep(|id| flat_ids.contains(&id));

        // The biggest files of the rows picked together.
        if many {
            let base = tree.node(self.open[self.open.len() - 1]).parent;
            let items: Vec<NodeId> = self.col.many.iter().copied().collect();
            self.picked_size = items.iter().map(|&id| tree.node(id).size).sum();
            let ids = self.merged_found.get(&tree, items, self.merged_limit + 1);
            self.merged_more = ids.len() > self.merged_limit;
            self.merged_rows = ids.iter().take(self.merged_limit).map(|&id| row(id, path_below(id, base))).collect();
        } else {
            self.merged_rows.clear();
            self.merged_more = false;
            self.picked_size = 0;
        }
        let merged_ids: HashSet<NodeId> = self.merged_rows.iter().map(|r| r.id).collect();
        self.merged.keep(|id| merged_ids.contains(&id));
        self.merged_focus = self.merged_focus.filter(|id| merged_ids.contains(id));

        // Breadcrumbs: from the scan root to the last opened item, or to the
        // folder of the rows picked together.
        self.crumbs.clear();
        for id in std::iter::once(tree.root()).chain(shown.iter().copied()) {
            self.crumbs.push((id, name_of(id)));
        }

        self.checked.retain(|&id| !tree.is_gone(id));
        self.checked_size = self.checked.iter().map(|&id| tree.node(id).size).sum();
    }
}

/// The chart centre for a path: the folder holding its last item, so that the item is on the first ring.
fn centre_of(path: &[NodeId]) -> NodeId {
    match path.len() {
        0 | 1 => 0,
        n => path[n - 2],
    }
}

/// The folder a path leads to: its last item, or the folder holding it when that is a file.
fn folder_of(tree: &Tree, path: &[NodeId]) -> NodeId {
    match path.last() {
        None => tree.root(),
        Some(&id) if tree.node(id).is_dir() => id,
        Some(&id) => tree.node(id).parent,
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
        assert_eq!(s.view_root, 0, "the selected folder is on the first ring of the chart");
        // One level deeper the chart centres on the folder holding the selection.
        s.select(1, 5);
        s.refresh();
        assert_eq!((s.view_root, s.view_name.as_str(), s.open.clone(), s.columns.len()), (1, "docs", vec![1, 5], 2));
        assert_eq!(names(&s.columns[0]), ["movie.mkv", "docs", "Small", "Hidden"]);
        assert_eq!(s.selected_file(), Some(5));
        s.go_parent();
        s.refresh();
        assert_eq!((s.view_root, s.open.clone(), s.selected_file()), (0, vec![1], None));
        s.select_path(6);
        s.refresh();
        assert_eq!(s.open, vec![1, 6]);
        s.go_crumb(1);
        assert_eq!(s.open, vec![1]);
        s.go_root();
        assert_eq!((s.view_root, s.open.clone()), (0, vec![]));
    }

    #[test]
    fn back_and_forward_follow_the_visited_folders() {
        let mut s = session();
        assert!(!s.go_history(false));
        s.select(0, 1);
        s.select(1, 5);
        s.select(1, 6);
        s.select(0, 3);
        assert_eq!((s.view_root, s.open.clone()), (0, vec![3]));
        // Files picked inside a folder are one step with it, and come back selected.
        assert!(s.go_history(false));
        assert_eq!((s.view_root, s.open.clone()), (1, vec![1, 6]));
        assert!(s.go_history(false));
        assert_eq!(s.open, Vec::<NodeId>::new());
        assert!(!s.can_go(false));
        assert!(s.go_history(true));
        assert!(s.go_history(true));
        assert_eq!(s.open, vec![3]);
        assert!(!s.go_history(true));
        // Moving somewhere else drops the steps ahead.
        s.go_history(false);
        s.go_root();
        assert!(!s.can_go(true));
        assert!(s.go_history(false));
        assert_eq!(s.open, vec![1, 6]);
    }

    #[test]
    fn history_skips_removed_items() {
        let mut s = session();
        s.select(0, 1);
        s.go_root();
        s.select(0, 3);
        s.scan.tree.write().remove(1);
        assert!(s.go_history(false));
        assert_eq!((s.view_root, s.open.clone()), (0, vec![]));
        assert!(!s.go_history(false), "the removed folder is not shown again");
        assert!(s.go_history(true));
        assert_eq!(s.open, vec![3]);
    }

    #[test]
    fn arrow_keys_walk_the_columns() {
        let mut s = session();
        s.refresh();
        s.step(Step::Down);
        assert_eq!(s.open, vec![2], "first row of the first column");
        s.step(Step::Down);
        assert_eq!(s.open, vec![1]);
        s.step(Step::Down);
        assert_eq!(s.open, vec![1], "groups are not rows");
        s.step(Step::Right);
        assert_eq!(s.open, vec![1, 5]);
        s.step(Step::Down);
        assert_eq!(s.open, vec![1, 6]);
        s.step(Step::Right);
        assert_eq!(s.open, vec![1, 6], "a file has no column");
        s.step(Step::Left);
        s.step(Step::Up);
        assert_eq!(s.open, vec![2]);
        s.step(Step::Left);
        s.step(Step::Left);
        assert_eq!(s.open, Vec::<NodeId>::new());
        // A run of key presses is a single step.
        assert!(s.go_history(false));
        assert_eq!(s.open, vec![2]);
        assert!(!s.can_go(false));
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

    fn ids(set: &BTreeSet<NodeId>) -> Vec<NodeId> {
        set.iter().copied().collect()
    }

    #[test]
    fn rows_picked_together_show_their_biggest_files() {
        let mut s = session();
        s.click(Pane::Column(0), 2, Click::One);
        assert_eq!((s.open.clone(), s.many()), (vec![2], false));
        // Ctrl (⌘) or Alt adds a row; the merged pane takes the place of its content.
        s.click(Pane::Column(0), 1, Click::Toggle);
        s.refresh();
        assert_eq!((s.open.clone(), ids(&s.col.many)), (vec![1], vec![1, 2]), "the row clicked last leads");
        assert_eq!(s.columns.len(), 1, "no column for the content of docs");
        assert_eq!(s.merged_rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["movie.mkv", "a.pdf", "b.pdf"]);
        assert_eq!((s.merged_rows[0].crumbs.clone(), s.merged_rows[1].crumbs.clone()), (vec!["root".to_string()], vec!["docs".to_string()]));
        assert_eq!(s.picked_size, 5_003_000_000);
        assert_eq!(s.crumbs.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["root"]);
        assert!(s.is_marked(1) && s.is_marked(2) && !s.is_marked(5));
        assert_eq!(s.picked_with(Pane::Column(0), 2).map(ids), Some(vec![1, 2]));
        // A file clicked in the merged pane is previewed and is what the menus act on.
        s.click(Pane::Merged, 5, Click::One);
        assert_eq!((s.selected_file(), s.target(), s.is_marked(5)), (Some(5), 5, true));
        // Taking a row out leaves the other one alone, with its own pane back.
        s.click(Pane::Column(0), 1, Click::Toggle);
        s.refresh();
        assert_eq!((s.open.clone(), s.many(), s.selected_file()), (vec![2], false, Some(2)));
        assert!(s.merged_rows.is_empty() && s.merged_focus.is_none());
        // Taking the last one out leaves nothing picked in the column.
        s.click(Pane::Column(0), 2, Click::Toggle);
        assert_eq!(s.open, Vec::<NodeId>::new());
    }

    #[test]
    fn shift_picks_a_range_and_escape_drops_it() {
        let mut s = session();
        s.toggle_group(0, GroupKind::Small);
        s.refresh();
        // Rows: movie.mkv, docs, then note.txt in the open group of small items.
        s.click(Pane::Column(0), 2, Click::One);
        s.click(Pane::Column(0), 4, Click::Range);
        assert_eq!((ids(&s.col.many), s.open.clone()), (vec![1, 2, 4], vec![4]));
        // The range keeps its start: a shorter one replaces it.
        s.click(Pane::Column(0), 1, Click::Range);
        assert_eq!(ids(&s.col.many), vec![1, 2]);
        s.click(Pane::Column(0), 4, Click::Toggle);
        s.click(Pane::Column(0), 2, Click::Toggle);
        assert_eq!(ids(&s.col.many), vec![1, 4]);
        // Shift with Ctrl adds the range from the row toggled last.
        s.click(Pane::Column(0), 2, Click::AddRange);
        assert_eq!(ids(&s.col.many), vec![1, 2, 4]);
        // Escape leaves the row clicked last alone.
        assert!(s.unpick());
        s.refresh();
        assert_eq!((s.open.clone(), s.many()), (vec![2], false));
        assert!(!s.unpick());
        // Shift with an arrow key picks the rows passed, and gives them back.
        s.extend(Step::Down);
        s.extend(Step::Down);
        assert_eq!((ids(&s.col.many), s.open.clone()), (vec![1, 2, 4], vec![4]));
        s.extend(Step::Up);
        assert_eq!(ids(&s.col.many), vec![1, 2]);
        // A plain arrow key moves from the row clicked last, alone.
        s.step(Step::Up);
        assert_eq!((s.open.clone(), s.many()), (vec![2], false));
        // Picks in a column do not mix with those of another.
        s.click(Pane::Column(0), 1, Click::One);
        s.refresh();
        s.click(Pane::Column(1), 5, Click::One);
        s.click(Pane::Column(0), 4, Click::Toggle);
        assert_eq!((ids(&s.col.many), s.open.clone()), (vec![1, 4], vec![4]));
    }

    #[test]
    fn removing_the_row_clicked_last_keeps_the_others_picked() {
        let mut s = session();
        s.toggle_group(0, GroupKind::Small);
        s.refresh();
        s.click(Pane::Column(0), 2, Click::One);
        s.click(Pane::Column(0), 4, Click::Range);
        // As `apply_results` does after a removal.
        let remove = |s: &mut Session, id: NodeId| {
            s.scan.tree.write().remove(id);
            s.mark_dirty();
            s.refresh();
        };
        remove(&mut s, 4);
        assert_eq!((s.open.clone(), ids(&s.col.many)), (vec![2], vec![1, 2]));
        // With one left, it is picked alone.
        remove(&mut s, 2);
        assert_eq!((s.open.clone(), s.many()), (vec![1], false));
    }

    #[test]
    fn rows_picked_in_biggest_files() {
        let mut s = session();
        s.tab = Tab::Biggest;
        s.mark_dirty();
        s.refresh();
        assert_eq!(s.biggest.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 7, 5, 6, 4]);
        s.click(Pane::Flat, 7, Click::One);
        s.click(Pane::Flat, 6, Click::Range);
        s.click(Pane::Flat, 4, Click::Toggle);
        assert_eq!((ids(&s.flat.many), s.focus), (vec![4, 5, 6, 7], Some(4)));
        assert!(s.has_picks() && s.is_marked(5) && !s.is_marked(2));
        assert_eq!(s.picked_with(Pane::Flat, 2), None);
        // Ticking them all skips what may not be ticked, and tells why.
        let picked = ids(&s.flat.many);
        assert_eq!(s.set_checks(&picked, true, false), Some(CheckError::Hidden));
        assert_eq!(s.checked.iter().copied().collect::<Vec<_>>(), vec![4, 5, 6]);
        assert_eq!(s.set_checks(&picked, false, false), None);
        assert!(s.checked.is_empty());
        // Escape, or a plain click, leaves one row picked.
        assert!(s.unpick());
        assert_eq!((s.has_picks(), s.focus), (false, Some(4)));
        s.click(Pane::Flat, 2, Click::Range);
        s.click(Pane::Flat, 6, Click::One);
        assert_eq!((s.has_picks(), s.focus), (false, Some(6)));
    }

    #[test]
    fn shift_click_on_checkboxes_ticks_a_range() {
        let mut s = session();
        s.toggle_group(0, GroupKind::Small);
        s.toggle_group(0, GroupKind::Hidden);
        s.refresh();
        // Rows: movie.mkv, docs, note.txt, then .cache in the group of hidden items.
        assert_eq!(s.tick(Pane::Column(0), 2, false, false), None);
        assert_eq!(s.tick(Pane::Column(0), 4, true, false), None);
        assert_eq!(s.checked.iter().copied().collect::<Vec<_>>(), vec![1, 2, 4]);
        // From a ticked box, Shift unticks the range.
        assert_eq!(s.tick(Pane::Column(0), 1, true, false), None);
        assert_eq!(s.checked.iter().copied().collect::<Vec<_>>(), vec![2]);
        // Items that may not be ticked are left out, and the reason told.
        assert_eq!(s.tick(Pane::Column(0), 3, true, false), Some(CheckError::Hidden));
        assert_eq!(s.checked.iter().copied().collect::<Vec<_>>(), vec![1, 2, 4]);
        // A range does not start from a box of another list.
        s.select(0, 1);
        s.refresh();
        assert_eq!(s.tick(Pane::Column(1), 6, true, false), Some(CheckError::ParentSelected));
        // Items inside a ticked folder go with it already.
        assert_eq!(s.set_checks(&[5, 2], true, false), None);
        assert_eq!(s.set_checks(&[5, 6], true, false), Some(CheckError::ParentSelected));
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

    #[test]
    fn size_filter_follows_removals() {
        let mut s = session();
        let settle = |s: &mut Session| {
            for _ in 0..500 {
                s.refresh();
                if !s.searching {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            panic!("search did not finish");
        };
        s.set_filter(Filter { min_size: 2_500_000, ..Filter::default() });
        settle(&mut s);
        let found = |s: &Session| s.search_rows.iter().any(|r| r.name == "docs");
        assert!(found(&s), "docs holds 3 MB");
        s.toggle_check(5, false).unwrap();
        let results: Vec<OpResult> = s.op_items(None).into_iter().map(|item| OpResult { item, error: None, renamed: None }).collect();
        s.apply_results(&results, true);
        settle(&mut s);
        assert!(!found(&s), "docs holds 1 MB once a.pdf is gone");
    }
}
