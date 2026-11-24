//! The MCP tools: scans kept in memory and the analyses run on them.

use crate::filter::{now, Age, Filter, Kind};
use crate::format::{count, date, parse_size, size};
use crate::scan::{start_scan, ScanHandle};
use crate::tree::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

pub enum CallError {
    UnknownTool,
    Failed(String),
}

impl From<String> for CallError {
    fn from(s: String) -> Self {
        CallError::Failed(s)
    }
}

type Outcome = Result<String, CallError>;

/// Longest a `scan` call waits before answering with partial results.
const MAX_WAIT_SECONDS: u64 = 300;
const DEFAULT_WAIT_SECONDS: u64 = 50;

struct Scan {
    root: PathBuf,
    handle: ScanHandle,
    /// Newest file date below each node, for the tree generation it was
    /// computed on.
    newest: Option<(u64, Vec<i64>)>,
}

pub struct Server {
    /// Paths that no scan may cover whole.
    deny: Vec<PathBuf>,
    scans: Vec<Scan>,
}

/// An absolute path with symbolic links resolved where it exists, and `.` and
/// `..` removed otherwise. `~` stands for the home folder.
pub fn absolute(text: &str) -> PathBuf {
    let expanded = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(text),
    };
    if let Ok(p) = std::fs::canonicalize(&expanded) {
        return p;
    }
    let mut out = PathBuf::new();
    for c in expanded.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

impl Server {
    pub fn new(deny: Vec<PathBuf>) -> Self {
        Server { deny, scans: Vec::new() }
    }

    pub fn call(&mut self, name: &str, args: &Value) -> Outcome {
        match name {
            "disks" => Ok(disks()),
            "scan" => self.scan(args),
            "list" => self.list(args),
            "biggest_files" => self.biggest_files(args),
            "search" => self.search(args),
            "by_type" => self.by_type(args),
            "stale_folders" => self.stale_folders(args),
            "duplicates" => self.duplicates(args),
            _ => Err(CallError::UnknownTool),
        }
    }

    fn scan(&mut self, args: &Value) -> Outcome {
        let path = path_arg(args)?;
        if !path.is_dir() {
            return Err(format!("{} is not a folder.", path.display()).into());
        }
        if let Some(d) = self.deny.iter().find(|d| d.starts_with(&path)) {
            return Err(format!(
                "Refused: scanning {} would cover {} whole, which this server is configured not to scan. Scan one of its subfolders instead.",
                path.display(),
                d.display()
            )
            .into());
        }
        let refresh = args.get("refresh").and_then(Value::as_bool).unwrap_or(false);
        let wait = args.get("wait_seconds").and_then(Value::as_u64).unwrap_or(DEFAULT_WAIT_SECONDS).min(MAX_WAIT_SECONDS);

        let existing = self.scans.iter().position(|s| s.root == path);
        let index = match existing {
            Some(i) if !refresh => i,
            _ => {
                // A new scan supersedes the scans of its subfolders.
                self.scans.retain(|s| {
                    let inside = s.root.starts_with(&path);
                    if inside {
                        s.handle.cancel();
                    }
                    !inside
                });
                self.scans.push(Scan { root: path.clone(), handle: start_scan(path.clone()), newest: None });
                self.scans.len() - 1
            }
        };

        let deadline = Instant::now() + Duration::from_secs(wait);
        while !self.scans[index].handle.is_done() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }

        let mut out = String::new();
        let h = &self.scans[index].handle;
        let elapsed = h.elapsed().as_secs_f64();
        {
            let t = h.tree.read();
            let root = t.node(t.root());
            if h.is_done() {
                let _ = writeln!(out, "Scan of {} complete in {elapsed:.1} s: {} in {} files.", path.display(), size(root.size), count(root.items as u64));
            } else {
                let _ = writeln!(
                    out,
                    "Scan of {} still running after {elapsed:.0} s, about {:.0} % read: {} in {} files so far. Call scan again with the same path to keep waiting; the other tools already work on the partial results.",
                    path.display(),
                    h.progress() * 100.0,
                    size(root.size),
                    count(root.items as u64)
                );
            }
            if t.denied_dirs > 0 {
                let _ = writeln!(out, "{} could not be read completely (permissions or errors); their content is missing.", plural(t.denied_dirs as u64, "folder"));
            }
        }
        if let Some(line) = filesystem_line(&path) {
            out.push_str(&line);
        }
        out.push('\n');
        if h.is_done() {
            self.ensure_newest(index);
            let s = &self.scans[index];
            let t = s.handle.tree.read();
            let newest = &s.newest.as_ref().expect("computed above").1;
            render(&t, newest, t.root(), 1, 15, "", &mut out);
        }
        Ok(out)
    }

    fn list(&mut self, args: &Value) -> Outcome {
        let path = path_arg(args)?;
        let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(1).clamp(1, 4) as usize;
        let limit = limit_arg(args, 20, 200);
        let (index, id) = self.locate(&path)?;
        self.ensure_newest(index);
        let s = &self.scans[index];
        let t = s.handle.tree.read();
        let newest = &s.newest.as_ref().expect("computed above").1;
        let mut out = partial_note(&s.handle);
        let n = t.node(id);
        if !n.is_dir() {
            let _ = writeln!(out, "{}: file, {}, modified {}.", path.display(), size(n.size), date(n.mtime));
            return Ok(out);
        }
        let _ = writeln!(out, "{}  {}  {} files  newest file {}", path.display(), size(n.size), count(n.items as u64), date(newest[id as usize]));
        render(&t, newest, id, depth, limit, "", &mut out);
        Ok(out)
    }

    fn biggest_files(&mut self, args: &Value) -> Outcome {
        let path = path_arg(args)?;
        let limit = limit_arg(args, 30, 500);
        let (index, id) = self.locate(&path)?;
        let s = &self.scans[index];
        let t = s.handle.tree.read();
        let mut out = partial_note(&s.handle);
        let files = t.biggest_files_in(&[id], limit);
        if files.is_empty() {
            out.push_str("No files.\n");
        }
        for f in files {
            let n = t.node(f);
            let _ = writeln!(out, "{:>9}  {}  {}", size(n.size), date(n.mtime), t.path(f).display());
        }
        Ok(out)
    }

    fn search(&mut self, args: &Value) -> Outcome {
        let path = path_arg(args)?;
        let limit = limit_arg(args, 30, 500);
        let name = args.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase();
        let folders = args.get("include_folders").and_then(Value::as_bool).unwrap_or(false);
        let mut filter = Filter { min_size: size_arg(args, "min_size")?.unwrap_or(0), ..Default::default() };
        if let Some(kinds) = args.get("kinds").and_then(Value::as_array) {
            for k in kinds {
                let k = k.as_str().unwrap_or("");
                let kind = kind_named(k).ok_or_else(|| format!("Unknown kind {k:?}: use video, image, audio, document or archive."))?;
                filter.kinds |= kind.bit();
            }
        }
        let older = args.get("older_than_days").and_then(Value::as_u64);
        let newer = args.get("newer_than_days").and_then(Value::as_u64);
        filter.age = match (older, newer) {
            (Some(_), Some(_)) => return Err("Give older_than_days or newer_than_days, not both.".to_string().into()),
            (Some(d), None) => Age::OlderThan(d.min(u32::MAX as u64) as u32),
            (None, Some(d)) => Age::NewerThan(d.min(u32::MAX as u64) as u32),
            (None, None) => Age::Any,
        };
        if name.is_empty() && filter.is_empty() {
            return Err("Give at least a name, kinds, min_size or a date bound.".to_string().into());
        }

        let (index, id) = self.locate(&path)?;
        let s = &self.scans[index];
        let t = s.handle.tree.read();
        let now = now();
        let mut found: Vec<(u64, NodeId)> = Vec::new();
        let mut stack = vec![id];
        while let Some(d) = stack.pop() {
            for c in t.children(d) {
                let n = t.node(c);
                if n.is_dir() {
                    stack.push(c);
                    if !folders {
                        continue;
                    }
                }
                if !filter.matches(n, now) {
                    continue;
                }
                if !name.is_empty() && !n.name.to_string_lossy().to_lowercase().contains(&name) {
                    continue;
                }
                found.push((n.size, c));
            }
        }
        let total: u64 = found.iter().filter(|(_, c)| !t.node(*c).is_dir()).map(|(s, _)| s).sum();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut out = partial_note(&s.handle);
        let _ = writeln!(out, "{}, {} in files.", plural(found.len() as u64, "match"), size(total));
        for &(_, c) in found.iter().take(limit) {
            let n = t.node(c);
            let slash = if n.is_dir() { "/" } else { "" };
            let _ = writeln!(out, "{:>9}  {}  {}{slash}", size(n.size), date(n.mtime), t.path(c).display());
        }
        if found.len() > limit {
            let _ = writeln!(out, "… {} more.", count((found.len() - limit) as u64));
        }
        Ok(out)
    }

    fn by_type(&mut self, args: &Value) -> Outcome {
        let path = path_arg(args)?;
        let limit = limit_arg(args, 25, 200);
        let (index, id) = self.locate(&path)?;
        let s = &self.scans[index];
        let t = s.handle.tree.read();
        let now = now();
        let mut by_ext: HashMap<String, (u64, u64)> = HashMap::new();
        let mut by_kind = [(0u64, 0u64); 5];
        let (mut total, mut files) = (0u64, 0u64);
        let kind_filters: Vec<Filter> = Kind::ALL.iter().map(|k| Filter { kinds: k.bit(), ..Default::default() }).collect();
        let mut stack = vec![id];
        while let Some(d) = stack.pop() {
            for c in t.children(d) {
                let n = t.node(c);
                if n.is_dir() && !n.is_package() {
                    stack.push(c);
                    continue;
                }
                total += n.size;
                files += 1;
                let e = by_ext.entry(extension(&n.name.to_string_lossy())).or_default();
                e.0 += n.size;
                e.1 += 1;
                if let Some(k) = kind_filters.iter().position(|f| f.matches(n, now)) {
                    by_kind[k].0 += n.size;
                    by_kind[k].1 += 1;
                }
            }
        }
        let mut out = partial_note(&s.handle);
        let pct = |b: u64| if total == 0 { 0.0 } else { b as f64 * 100.0 / total as f64 };
        let _ = writeln!(out, "{} in {} files under {}.\n\nBy category:", size(total), count(files), path.display());
        let mut kinds: Vec<(usize, (u64, u64))> = by_kind.iter().copied().enumerate().collect();
        kinds.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        for (k, (bytes, n)) in kinds {
            let _ = writeln!(out, "{:>9}  {:>5.1} %  {}  ({} files)", size(bytes), pct(bytes), Kind::ALL[k].label(), count(n));
        }
        let mut exts: Vec<(String, (u64, u64))> = by_ext.into_iter().collect();
        exts.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
        let _ = writeln!(out, "\nBy extension:");
        for (ext, (bytes, n)) in exts.iter().take(limit) {
            let _ = writeln!(out, "{:>9}  {:>5.1} %  {ext}  ({} files)", size(*bytes), pct(*bytes), count(*n));
        }
        Ok(out)
    }

    fn stale_folders(&mut self, args: &Value) -> Outcome {
        let path = path_arg(args)?;
        let limit = limit_arg(args, 20, 200);
        let days = args.get("older_than_days").and_then(Value::as_u64).unwrap_or(365);
        let min_size = size_arg(args, "min_size")?.unwrap_or(1_000_000_000);
        let (index, id) = self.locate(&path)?;
        self.ensure_newest(index);
        let s = &self.scans[index];
        let t = s.handle.tree.read();
        let newest = &s.newest.as_ref().expect("computed above").1;
        let cutoff = now() - days as i64 * 86_400;

        // Biggest folders whose files are all older than the cutoff, without
        // listing a folder inside one already listed.
        let mut found: Vec<NodeId> = Vec::new();
        let mut stack = vec![id];
        while let Some(d) = stack.pop() {
            for c in t.children(d) {
                let n = t.node(c);
                if !n.is_dir() || n.size < min_size {
                    continue;
                }
                let last = newest[c as usize];
                if last > 0 && last < cutoff && !t.unscanned(c) {
                    found.push(c);
                } else {
                    stack.push(c);
                }
            }
        }
        found.sort_by(|&a, &b| t.node(b).size.cmp(&t.node(a).size));
        let total: u64 = found.iter().map(|&c| t.node(c).size).sum();
        let mut out = partial_note(&s.handle);
        let _ = writeln!(
            out,
            "{} of {} or more whose newest file is older than {days} days: {} in all.",
            plural(found.len() as u64, "folder"),
            size(min_size),
            size(total)
        );
        for &c in found.iter().take(limit) {
            let n = t.node(c);
            let _ = writeln!(out, "{:>9}  newest file {}  {:>9} files  {}/", size(n.size), date(newest[c as usize]), count(n.items as u64), t.path(c).display());
        }
        if found.len() > limit {
            let _ = writeln!(out, "… {} more.", count((found.len() - limit) as u64));
        }
        Ok(out)
    }

    fn duplicates(&mut self, args: &Value) -> Outcome {
        let path = path_arg(args)?;
        let limit = limit_arg(args, 20, 200);
        let file_min = size_arg(args, "min_file_size")?.unwrap_or(100_000_000);
        let folder_min = size_arg(args, "min_folder_size")?.unwrap_or(1_000_000_000);
        let (index, id) = self.locate(&path)?;
        let s = &self.scans[index];
        let t = s.handle.tree.read();

        // Size, file count and the names directly inside: copies keep their
        // names, unrelated folders of equal size (disk images) do not.
        let mut folders: HashMap<(u64, u32, u64), Vec<NodeId>> = HashMap::new();
        let mut files: HashMap<(u64, String), Vec<NodeId>> = HashMap::new();
        let mut stack = vec![id];
        while let Some(d) = stack.pop() {
            for c in t.children(d) {
                let n = t.node(c);
                if n.is_dir() && !n.is_package() {
                    stack.push(c);
                    if n.size >= folder_min && n.items > 0 && !t.unscanned(c) {
                        folders.entry((n.size, n.items, names_hash(&t, c))).or_default().push(c);
                    }
                } else if n.size >= file_min {
                    files.entry((n.size, n.name.to_string_lossy().to_lowercase())).or_default().push(c);
                }
            }
        }

        let mut out = partial_note(&s.handle);
        // Folder groups: a folder holding a single subfolder has the same
        // size and file count as it, so keep only folders unrelated to each
        // other, and skip the copies nested in a group already reported.
        let mut groups: Vec<Vec<NodeId>> = folders
            .into_values()
            .map(|mut g| {
                let all = g.clone();
                g.retain(|&a| !all.iter().any(|&b| b != a && t.is_ancestor(a, b)));
                g
            })
            .filter(|g| g.len() > 1)
            .collect();
        groups.sort_by(|a, b| t.node(b[0]).size.cmp(&t.node(a[0]).size));
        let mut covered: HashSet<NodeId> = HashSet::new();
        let inside_covered = |id: NodeId, covered: &HashSet<NodeId>| t.chain(id).iter().any(|a| covered.contains(a));
        let mut shown = 0;
        let mut wasted = 0u64;
        let _ = writeln!(out, "Folders with the same size, file count and names inside (probable copies, content not compared):");
        for g in &groups {
            if g.iter().all(|&m| inside_covered(m, &covered)) {
                continue;
            }
            covered.extend(g.iter().copied());
            let n = t.node(g[0]);
            wasted += n.size * (g.len() as u64 - 1);
            if shown < limit {
                let _ = writeln!(out, "{:>9} each, {}, {} copies:", size(n.size), plural(n.items as u64, "file"), g.len());
                for &m in g {
                    let _ = writeln!(out, "             {}/", t.path(m).display());
                }
            }
            shown += 1;
        }
        if shown == 0 {
            out.push_str("  none\n");
        } else if shown > limit {
            let _ = writeln!(out, "… {} more groups.", count((shown - limit) as u64));
        }
        let _ = writeln!(out, "Space held by the extra folder copies: {}.\n", size(wasted));

        let mut file_groups: Vec<Vec<NodeId>> = files
            .into_values()
            .map(|mut g| {
                g.retain(|&m| !inside_covered(m, &covered));
                g
            })
            .filter(|g| g.len() > 1)
            .collect();
        file_groups.sort_by(|a, b| {
            let wa = t.node(a[0]).size * (a.len() as u64 - 1);
            let wb = t.node(b[0]).size * (b.len() as u64 - 1);
            wb.cmp(&wa)
        });
        let file_wasted: u64 = file_groups.iter().map(|g| t.node(g[0]).size * (g.len() as u64 - 1)).sum();
        let _ = writeln!(
            out,
            "Files of {} or more with the same name and size, outside the folders above (content not compared):",
            size(file_min)
        );
        if file_groups.is_empty() {
            out.push_str("  none\n");
        }
        for g in file_groups.iter().take(limit) {
            let n = t.node(g[0]);
            let _ = writeln!(out, "{:>9} each, {} copies of {}:", size(n.size), g.len(), n.display_name());
            for &m in g.iter().take(6) {
                let _ = writeln!(out, "             {}", t.path(m).display());
            }
            if g.len() > 6 {
                let _ = writeln!(out, "             … {} more", g.len() - 6);
            }
        }
        if file_groups.len() > limit {
            let _ = writeln!(out, "… {} more groups.", count((file_groups.len() - limit) as u64));
        }
        let _ = writeln!(out, "Space held by the extra file copies: {}.", size(file_wasted));
        Ok(out)
    }

    /// The scan holding `path` (the deepest one if scans are nested) and the
    /// node of `path` in it.
    fn locate(&self, path: &Path) -> Result<(usize, NodeId), String> {
        let Some((index, s)) = self
            .scans
            .iter()
            .enumerate()
            .filter(|(_, s)| path.starts_with(&s.root))
            .max_by_key(|(_, s)| s.root.components().count())
        else {
            return Err(format!("{} has not been scanned: call scan on it, or on a folder above it, first.", path.display()));
        };
        let t = s.handle.tree.read();
        let mut id = t.root();
        let rest = path.strip_prefix(&s.root).expect("filtered on starts_with");
        for part in rest.components() {
            let name = part.as_os_str();
            match t.children(id).find(|&c| &*t.node(c).name == name) {
                Some(c) => id = c,
                None => {
                    let why = if t.unscanned(id) {
                        format!("{} is on another filesystem or could not be read", t.path(id).display())
                    } else if s.handle.is_done() {
                        "it was created after the scan, or it is not a folder".to_string()
                    } else {
                        "the scan has not reached it yet".to_string()
                    };
                    return Err(format!("{} is not in the scan of {}: {why}. Scan it, or scan again with refresh: true.", path.display(), s.root.display()));
                }
            }
        }
        Ok((index, id))
    }

    fn ensure_newest(&mut self, index: usize) {
        let s = &mut self.scans[index];
        let tree = s.handle.tree.clone();
        let t = tree.read();
        if s.newest.as_ref().is_some_and(|(g, v)| *g == t.generation && v.len() == t.nodes.len()) {
            return;
        }
        // A date in the future (wrong clock, bogus archive) would hide every
        // real date above it.
        let latest = now() + 86_400;
        let mut v: Vec<i64> = t.nodes.iter().map(|n| if n.is_dir() || n.mtime > latest { 0 } else { n.mtime }).collect();
        // Children always come after their parent in the arena.
        for i in (1..v.len()).rev() {
            let p = t.nodes[i].parent as usize;
            if v[i] > v[p] {
                v[p] = v[i];
            }
        }
        s.newest = Some((t.generation, v));
    }
}

/// Children of `id`, biggest first, down to `depth` levels.
fn render(t: &Tree, newest: &[i64], id: NodeId, depth: usize, limit: usize, indent: &str, out: &mut String) {
    let parent = t.node(id).size.max(1);
    let kids = t.sorted_children(id);
    for (k, &c) in kids.iter().enumerate() {
        if k == limit {
            let rest: u64 = kids[k..].iter().map(|&c| t.node(c).size).sum();
            let _ = writeln!(out, "{indent}{:>9}  {:>5.1} %  … {} more items", size(rest), rest as f64 * 100.0 / parent as f64, count((kids.len() - k) as u64));
            break;
        }
        let n = t.node(c);
        let pct = n.size as f64 * 100.0 / parent as f64;
        let mut notes = String::new();
        if n.flags & F_OTHER_FS != 0 {
            notes.push_str("  [other filesystem, not scanned]");
        } else if n.flags & F_DENIED != 0 {
            notes.push_str("  [partly unreadable]");
        }
        if n.flags & F_SYMLINK != 0 {
            notes.push_str("  [symbolic link]");
        }
        if n.is_dir() {
            let _ = writeln!(
                out,
                "{indent}{:>9}  {pct:>5.1} %  {}/  {} files, newest {}{notes}",
                size(n.size),
                n.display_name(),
                count(n.items as u64),
                date(newest.get(c as usize).copied().unwrap_or(0))
            );
            if depth > 1 && n.child_count > 0 {
                render(t, newest, c, depth - 1, limit, &format!("{indent}    "), out);
            }
        } else {
            let _ = writeln!(out, "{indent}{:>9}  {pct:>5.1} %  {}  modified {}{notes}", size(n.size), n.display_name(), date(n.mtime));
        }
    }
}

/// Hash of the sorted names directly inside a folder.
fn names_hash(t: &Tree, id: NodeId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut names: Vec<&std::ffi::OsStr> = t.children(id).map(|c| &*t.node(c).name).collect();
    names.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut h);
    h.finish()
}

/// `1 folder`, `2 folders`, `3 matches`.
fn plural(n: u64, word: &str) -> String {
    let suffix = match (n, word.ends_with("ch") || word.ends_with('s')) {
        (1, _) => "",
        (_, true) => "es",
        _ => "s",
    };
    format!("{} {word}{suffix}", count(n))
}

fn partial_note(h: &ScanHandle) -> String {
    if h.is_done() {
        String::new()
    } else {
        format!("Scan still running (about {:.0} % read): figures are partial.\n", h.progress() * 100.0)
    }
}

fn path_arg(args: &Value) -> Result<PathBuf, String> {
    let text = args.get("path").and_then(Value::as_str).ok_or("Missing argument: path (an absolute folder path).")?;
    if !(text.starts_with('/') || text.starts_with('~') || Path::new(text).is_absolute()) {
        return Err(format!("{text} is not an absolute path."));
    }
    Ok(absolute(text))
}

fn limit_arg(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit").and_then(Value::as_u64).map(|l| l as usize).unwrap_or(default).clamp(1, max)
}

/// A size given as a number of bytes or as text such as `500MB`.
fn size_arg(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64().map(|f| f.max(0.0) as u64)),
        Some(Value::String(s)) => parse_size(s).map(Some).ok_or_else(|| format!("{key}: cannot read {s:?} as a size; use bytes or e.g. \"500MB\", \"2GB\".")),
        Some(other) => Err(format!("{key}: expected a size, got {other}.")),
    }
}

fn kind_named(name: &str) -> Option<Kind> {
    Some(match name.to_ascii_lowercase().trim_end_matches('s') {
        "video" => Kind::Video,
        "image" | "picture" => Kind::Image,
        "audio" => Kind::Audio,
        "document" => Kind::Document,
        "archive" => Kind::Archive,
        _ => return None,
    })
}

/// Lower-case extension, or `(none)`; a leading dot alone does not count.
fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 12 && !ext.contains(' ') => format!(".{}", ext.to_lowercase()),
        _ => "(none)".into(),
    }
}

fn disks() -> String {
    let list = sysinfo::Disks::new_with_refreshed_list();
    let mut disks: Vec<_> = list.list().iter().collect();
    disks.sort_by(|a, b| a.mount_point().cmp(b.mount_point()));
    let mut seen = HashSet::new();
    let mut out = String::from("     size       used       free  used %  filesystem  mount point\n");
    for d in disks {
        // Container layers and memory filesystems say nothing about disk space.
        let fs = d.file_system().to_string_lossy();
        if d.total_space() == 0 || ["overlay", "squashfs", "tmpfs", "devtmpfs"].contains(&fs.as_ref()) {
            continue;
        }
        if !seen.insert(d.mount_point().to_path_buf()) {
            continue;
        }
        let total = d.total_space();
        let free = d.available_space();
        let used = total.saturating_sub(free);
        let pct = if total == 0 { 0.0 } else { used as f64 * 100.0 / total as f64 };
        let _ = writeln!(
            out,
            "{:>9}  {:>9}  {:>9}  {pct:>5.1} %  {:<10}  {}",
            size(total),
            size(used),
            size(free),
            d.file_system().to_string_lossy(),
            d.mount_point().display()
        );
    }
    out
}

/// Size and free space of the filesystem holding `path`.
fn filesystem_line(path: &Path) -> Option<String> {
    let list = sysinfo::Disks::new_with_refreshed_list();
    let d = list.list().iter().filter(|d| path.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().components().count())?;
    let total = d.total_space();
    let free = d.available_space();
    Some(format!(
        "Filesystem {} mounted on {}: {} in all, {} free ({:.1} % used).\n",
        d.name().to_string_lossy(),
        d.mount_point().display(),
        size(total),
        size(free),
        if total == 0 { 0.0 } else { (total - free.min(total)) as f64 * 100.0 / total as f64 }
    ))
}

pub fn definitions() -> Value {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    let path = json!({ "type": "string", "description": "Absolute path of a folder (or file) inside a scanned folder. ~ stands for the home folder." });
    let limit = |default: u64| json!({ "type": "integer", "minimum": 1, "description": format!("Most lines to return (default {default}).") });
    let size_param = |what: &str, default: &str| {
        json!({ "type": ["string", "number"], "description": format!("{what}, as bytes or text such as \"500MB\" or \"2GB\" (default {default}).") })
    };
    let tools = vec![
        json!({
            "name": "disks",
            "title": "Disks",
            "description": "Lists the mounted filesystems with their size, used and free space. Start here to see which disk is short of space.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only,
        }),
        json!({
            "name": "scan",
            "title": "Scan a folder",
            "description": "Scans a folder and keeps the result in memory for the other tools. Waits up to wait_seconds, then answers with the progress; call again with the same path to keep waiting. A finished scan is reused until refresh is true. The scan stays on the folder's filesystem and does not follow symbolic links. Returns the biggest items at the top of the folder.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path of the folder to scan. ~ stands for the home folder." },
                    "refresh": { "type": "boolean", "description": "Scan again even if this folder was already scanned (default false)." },
                    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT_SECONDS, "description": format!("How long to wait for the scan to finish before answering (default {DEFAULT_WAIT_SECONDS}).") },
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "list",
            "title": "List the biggest items",
            "description": "Shows what a scanned folder holds, biggest first, with each item's share, file count and newest file date. depth > 1 opens the subfolders as an indented tree.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "depth": { "type": "integer", "minimum": 1, "maximum": 4, "description": "Folder levels to show (default 1)." },
                    "limit": limit(20),
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "biggest_files",
            "title": "Biggest files",
            "description": "The biggest files anywhere below a scanned folder, with their modification date and full path.",
            "inputSchema": { "type": "object", "properties": { "path": path, "limit": limit(30) }, "required": ["path"] },
            "annotations": read_only,
        }),
        json!({
            "name": "search",
            "title": "Search files",
            "description": "Finds files below a scanned folder by part of their name, category, minimum size and modification date, biggest first, with the total size of the matches.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "name": { "type": "string", "description": "Part of the name, case-insensitive." },
                    "kinds": { "type": "array", "items": { "type": "string", "enum": ["video", "image", "audio", "document", "archive"] }, "description": "File categories; archive includes disk images (iso, img, vmdk, qcow2...)." },
                    "min_size": size_param("Smallest size to report", "none"),
                    "older_than_days": { "type": "integer", "minimum": 0, "description": "Only files not modified for this many days." },
                    "newer_than_days": { "type": "integer", "minimum": 0, "description": "Only files modified within this many days." },
                    "include_folders": { "type": "boolean", "description": "Also report folders matching the name and min_size (default false)." },
                    "limit": limit(30),
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "by_type",
            "title": "Space by file type",
            "description": "Splits the space below a scanned folder by category (video, image, audio, document, archive) and by file extension.",
            "inputSchema": { "type": "object", "properties": { "path": path, "limit": limit(25) }, "required": ["path"] },
            "annotations": read_only,
        }),
        json!({
            "name": "stale_folders",
            "title": "Untouched folders",
            "description": "The biggest folders below a scanned folder whose newest file is older than a number of days: good candidates for archiving. A folder inside a listed one is not listed again.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "older_than_days": { "type": "integer", "minimum": 0, "description": "Age of the newest file, in days (default 365)." },
                    "min_size": size_param("Smallest folder to report", "1GB"),
                    "limit": limit(20),
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "duplicates",
            "title": "Probable copies",
            "description": "Probable duplicates below a scanned folder: folders with the same size and file count, then files with the same name and size outside those folders. Contents are not compared: confirm before suggesting a deletion.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": path,
                    "min_folder_size": size_param("Smallest folder to compare", "1GB"),
                    "min_file_size": size_param("Smallest file to compare", "100MB"),
                    "limit": limit(20),
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
    ];
    Value::Array(tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = fs::File::create(p).unwrap();
        f.write_all(&vec![7u8; bytes]).unwrap();
        f.sync_all().unwrap();
    }

    fn call(s: &mut Server, name: &str, args: Value) -> String {
        match s.call(name, &args) {
            Ok(t) => t,
            Err(CallError::Failed(t)) => panic!("{name} failed: {t}"),
            Err(CallError::UnknownTool) => panic!("unknown tool {name}"),
        }
    }

    fn sample() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("photos/a.jpg"), 300_000);
        write(&root.join("photos/b.jpg"), 200_000);
        write(&root.join("copy/photos/a.jpg"), 300_000);
        write(&root.join("copy/photos/b.jpg"), 200_000);
        write(&root.join("video.mkv"), 600_000);
        write(&root.join("cards/le/le.bin"), 400_000);
        write(&root.join("cards/pro/pro.bin"), 400_000);
        write(&root.join("notes.txt"), 10);
        let p = absolute(&root.to_string_lossy()).to_string_lossy().into_owned();
        (dir, p)
    }

    #[test]
    fn scan_then_analyse() {
        let (_dir, root) = sample();
        let mut s = Server::new(Vec::new());
        let out = call(&mut s, "scan", json!({ "path": root, "wait_seconds": 20 }));
        assert!(out.contains("complete"), "{out}");
        assert!(out.contains("video.mkv"), "{out}");

        let out = call(&mut s, "list", json!({ "path": root, "depth": 2 }));
        let photos = out.find("photos/").unwrap();
        assert!(out.find("copy/").unwrap() < photos, "biggest first: {out}");

        let out = call(&mut s, "biggest_files", json!({ "path": root, "limit": 1 }));
        assert!(out.contains("video.mkv") && !out.contains("a.jpg"), "{out}");

        let out = call(&mut s, "search", json!({ "path": root, "kinds": ["image"] }));
        assert!(out.starts_with("4 matches"), "{out}");
        let out = call(&mut s, "search", json!({ "path": format!("{root}/copy"), "name": "A." }));
        assert!(out.starts_with("1 match,"), "{out}");

        let out = call(&mut s, "by_type", json!({ "path": root }));
        assert!(out.contains(".jpg  (4 files)"), "{out}");

        let out = call(&mut s, "duplicates", json!({ "path": root, "min_folder_size": 1, "min_file_size": 1 }));
        assert!(out.contains("2 copies:"), "{out}");
        assert!(!out.contains("cards/le"), "same size but other names: {out}");
        // The copied folder is reported once, not again file by file.
        assert!(out.contains("outside the folders above (content not compared):\n  none"), "{out}");

        let out = call(&mut s, "stale_folders", json!({ "path": root, "older_than_days": 1, "min_size": 1 }));
        assert!(out.starts_with("0 folders of"), "files written today are not a day old: {out}");
    }

    #[test]
    fn denied_roots_and_their_parents_are_refused() {
        let (_dir, root) = sample();
        let mut s = Server::new(vec![absolute(&format!("{root}/photos"))]);
        for path in [root.clone(), format!("{root}/photos")] {
            match s.call("scan", &json!({ "path": path })) {
                Err(CallError::Failed(t)) => assert!(t.starts_with("Refused"), "{t}"),
                _ => panic!("scan of {path} should be refused"),
            }
        }
        let out = call(&mut s, "scan", json!({ "path": format!("{root}/copy"), "wait_seconds": 20 }));
        assert!(out.contains("complete"), "{out}");
    }

    #[test]
    fn unscanned_paths_are_explained() {
        let (_dir, root) = sample();
        let mut s = Server::new(Vec::new());
        assert!(matches!(s.call("list", &json!({ "path": root })), Err(CallError::Failed(t)) if t.contains("has not been scanned")));
        call(&mut s, "scan", json!({ "path": format!("{root}/copy"), "wait_seconds": 20 }));
        assert!(matches!(s.call("list", &json!({ "path": format!("{root}/copy/nope") })), Err(CallError::Failed(t)) if t.contains("not in the scan")));
        assert!(matches!(s.call("list", &json!({ "path": "relative/path" })), Err(CallError::Failed(t)) if t.contains("not an absolute path")));
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("a.TAR.GZ"), ".gz");
        assert_eq!(extension(".bashrc"), "(none)");
        assert_eq!(extension("Makefile"), "(none)");
    }
}
