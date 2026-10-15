//! Sunburst chart: layout of the visible part of the tree, painting and
//! hit-testing. Angles are in radians, 0 at 12 o'clock, growing clockwise.

use crate::theme::Palette;
use crate::tree::{NodeId, Tree};
use egui::{pos2, Color32, Mesh, Pos2, Shape, Stroke};
use std::f32::consts::TAU;

/// Segments narrower than this are merged in a "small items" segment.
const MIN_ANGLE: f32 = 0.006;
pub const MAX_RINGS: u8 = 8;
const MAX_SEGMENTS: usize = 6000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SegKind {
    Node,
    /// Aggregate of the children of `id` that are too small to draw.
    Small,
    /// Free space of the volume.
    Free,
    /// Used space of the volume that the scan could not attribute.
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub id: NodeId,
    pub kind: SegKind,
    /// Ring number, 1 = children of the chart root.
    pub depth: u8,
    pub a0: f32,
    pub a1: f32,
    pub size: u64,
    pub hidden: bool,
}

#[derive(Default)]
pub struct Layout {
    pub segs: Vec<Seg>,
    pub rings: u8,
    /// Angular range that receives the rainbow gradient.
    hue_from: f32,
    hue_to: f32,
}

pub fn build(tree: &Tree, root: NodeId, free: u64, other: u64) -> Layout {
    let mut l = Layout { segs: Vec::new(), rings: 0, hue_from: 0.0, hue_to: TAU };
    let scanned = tree.node(root).size;
    let total = scanned as f64 + free as f64 + other as f64;
    if total <= 0.0 {
        return l;
    }
    let data_end = (scanned as f64 / total * TAU as f64) as f32;
    // Breadth first, so that the segment budget goes to the inner rings of
    // every branch before the outer rings of the first ones.
    let mut queue = std::collections::VecDeque::from([(root, 0.0f32, data_end, 1u8, false)]);
    while let Some((parent, a0, a1, depth, hidden)) = queue.pop_front() {
        if l.segs.len() >= MAX_SEGMENTS {
            break;
        }
        children(tree, parent, a0, a1, depth, hidden, &mut l, &mut queue);
    }
    l.hue_from = l
        .segs
        .iter()
        .filter(|s| s.depth == 1 && s.hidden && s.kind == SegKind::Node)
        .map(|s| s.a1)
        .fold(0.0, f32::max);
    l.hue_to = data_end.max(l.hue_from + 0.001);
    let mut a = data_end;
    for (size, kind) in [(other, SegKind::Other), (free, SegKind::Free)] {
        let span = (size as f64 / total * TAU as f64) as f32;
        if span > 0.0005 {
            l.segs.push(Seg { id: root, kind, depth: 1, a0: a, a1: a + span, size, hidden: false });
            l.rings = l.rings.max(1);
        }
        a += span;
    }
    l
}

type Pending = std::collections::VecDeque<(NodeId, f32, f32, u8, bool)>;

#[allow(clippy::too_many_arguments)]
fn children(tree: &Tree, parent: NodeId, a0: f32, a1: f32, depth: u8, in_hidden: bool, l: &mut Layout, queue: &mut Pending) {
    let psize = tree.node(parent).size;
    if psize == 0 || a1 - a0 < MIN_ANGLE {
        return;
    }
    let scale = (a1 - a0) as f64 / psize as f64;
    let min_size = (MIN_ANGLE as f64 / scale) as u64;
    let mut small = 0u64;
    let mut big: Vec<(NodeId, u64, bool)> = Vec::new();
    for c in tree.children(parent) {
        let n = tree.node(c);
        if n.size >= min_size && n.size > 0 {
            big.push((c, n.size, in_hidden || n.is_hidden()));
        } else {
            small += n.size;
        }
    }
    // Hidden items lead the first ring, like a separate category.
    big.sort_by(|a, b| {
        let key = |x: &(NodeId, u64, bool)| !(depth == 1 && x.2);
        key(a).cmp(&key(b)).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0))
    });
    let mut a = a0;
    for (id, size, hidden) in big {
        let span = (size as f64 * scale) as f32;
        let end = (a + span).min(a1);
        l.segs.push(Seg { id, kind: SegKind::Node, depth, a0: a, a1: end, size, hidden });
        l.rings = l.rings.max(depth);
        if tree.node(id).is_dir() && depth < MAX_RINGS && l.segs.len() < MAX_SEGMENTS {
            queue.push_back((id, a, end, depth + 1, hidden));
        }
        a = end;
    }
    let span = (small as f64 * scale) as f32;
    if span >= MIN_ANGLE && a < a1 {
        l.segs.push(Seg {
            id: parent,
            kind: SegKind::Small,
            depth,
            a0: a,
            a1: (a + span).min(a1),
            size: small,
            hidden: in_hidden,
        });
        l.rings = l.rings.max(depth);
    }
}

#[derive(Clone, Copy)]
pub struct Geo {
    pub center: Pos2,
    /// Radius of the centre disc.
    pub r0: f32,
    /// Thickness of one ring.
    pub ring: f32,
}

impl Geo {
    pub fn fit(rect: egui::Rect, rings: u8) -> Geo {
        let radius = (rect.width().min(rect.height()) * 0.47).max(40.0);
        let r0 = radius * 0.34;
        Geo { center: rect.center(), r0, ring: (radius - r0) / rings.max(5) as f32 }
    }

    fn point(&self, a: f32, r: f32) -> Pos2 {
        pos2(self.center.x + r * a.sin(), self.center.y - r * a.cos())
    }

    fn radii(&self, depth: u8) -> (f32, f32) {
        let inner = self.r0 + (depth - 1) as f32 * self.ring;
        (inner + 1.0, inner + self.ring)
    }

    /// Where a popover for `seg` should point.
    pub fn anchor(&self, seg: &Seg) -> Pos2 {
        let (ri, ro) = self.radii(seg.depth);
        self.point((seg.a0 + seg.a1) * 0.5, (ri + ro) * 0.5)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    Center,
    Seg(usize),
}

pub fn hit(l: &Layout, g: &Geo, pos: Pos2) -> Hit {
    let d = pos - g.center;
    let r = d.length();
    if r <= g.r0 {
        return Hit::Center;
    }
    let depth = ((r - g.r0) / g.ring).floor() as i32 + 1;
    if depth < 1 || depth > l.rings as i32 {
        return Hit::None;
    }
    let mut a = d.x.atan2(-d.y);
    if a < 0.0 {
        a += TAU;
    }
    l.segs
        .iter()
        .position(|s| s.depth as i32 == depth && a >= s.a0 && a < s.a1)
        .map(Hit::Seg)
        .unwrap_or(Hit::None)
}

/// HSV to sRGB, without the linear-light detour that washes colours out.
fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

pub fn seg_color(l: &Layout, seg: &Seg, index: usize, pal: &Palette, checked: bool) -> Color32 {
    let shade = 1.0 - 0.05 * (index % 2) as f32;
    match seg.kind {
        SegKind::Free => return pal.seg_free,
        SegKind::Other => return pal.seg_other,
        _ => {}
    }
    if checked {
        return pal.seg_checked.gamma_multiply(shade);
    }
    if seg.hidden {
        return pal.seg_hidden.gamma_multiply(1.0 - 0.035 * ((index + seg.depth as usize) % 3) as f32);
    }
    let mid = (seg.a0 + seg.a1) * 0.5;
    let t = ((mid - l.hue_from) / (l.hue_to - l.hue_from)).clamp(0.0, 1.0);
    // Red at the start, through yellow, green and blue, to violet.
    let hue = t * 0.78;
    let sat = if seg.kind == SegKind::Small { 0.4 } else { 0.78 - 0.07 * ((seg.depth - 1) % 3) as f32 };
    let val = if pal.dark { 0.97 } else { 0.92 };
    hsv(hue, sat, val * shade)
}

/// Appends the annular sector of `seg` to `mesh`.
pub fn add_segment(mesh: &mut Mesh, g: &Geo, seg: &Seg, color: Color32) {
    let (ri, ro) = g.radii(seg.depth);
    let span = seg.a1 - seg.a0;
    // Half a pixel of background between neighbours acts as a separator.
    let gap = (0.5 / ((ri + ro) * 0.5)).min(span * 0.2);
    let (a0, a1) = (seg.a0 + gap, seg.a1 - gap);
    let steps = (((a1 - a0) / 0.03).ceil() as usize).max(1);
    let base = mesh.vertices.len() as u32;
    for i in 0..=steps {
        let a = a0 + (a1 - a0) * i as f32 / steps as f32;
        mesh.colored_vertex(g.point(a, ri), color);
        mesh.colored_vertex(g.point(a, ro), color);
    }
    for i in 0..steps as u32 {
        let v = base + i * 2;
        mesh.add_triangle(v, v + 1, v + 2);
        mesh.add_triangle(v + 1, v + 3, v + 2);
    }
}

pub fn outline(g: &Geo, seg: &Seg) -> Vec<Pos2> {
    let (ri, ro) = g.radii(seg.depth);
    let steps = (((seg.a1 - seg.a0) / 0.03).ceil() as usize).max(1);
    let mut pts = Vec::with_capacity(steps * 2 + 2);
    for i in 0..=steps {
        pts.push(g.point(seg.a0 + (seg.a1 - seg.a0) * i as f32 / steps as f32, ro));
    }
    for i in (0..=steps).rev() {
        pts.push(g.point(seg.a0 + (seg.a1 - seg.a0) * i as f32 / steps as f32, ri));
    }
    pts
}

/// Paints the centre disc with a soft radial gradient.
pub fn paint_center(p: &egui::Painter, g: &Geo, pal: &Palette, hovered: bool) {
    let mut mesh = Mesh::default();
    let (inner, outer) = if hovered { (pal.center_a.gamma_multiply(1.12), pal.center_a) } else { (pal.center_a, pal.center_b) };
    mesh.colored_vertex(g.center - egui::vec2(g.r0 * 0.15, g.r0 * 0.25), inner);
    let n = 96;
    for i in 0..=n {
        mesh.colored_vertex(g.point(i as f32 / n as f32 * TAU, g.r0 - 3.0), outer);
    }
    for i in 1..=n as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    p.circle_filled(g.center + egui::vec2(0.0, 2.0), g.r0 + 1.0, Color32::from_black_alpha(50));
    p.add(Shape::mesh(mesh));
    p.circle_stroke(g.center, g.r0 - 2.0, Stroke::new(2.0_f32, pal.center_ring));
}

/// Decorative artwork of the start screen: a rainbow ring with faded rays.
pub fn paint_idle_art(p: &egui::Painter, center: Pos2, radius: f32, pal: &Palette) {
    let g = Geo { center, r0: radius * 0.5, ring: radius * 0.22 };
    let n = 44usize;
    let mut rays = Mesh::default();
    let mut ring = Mesh::default();
    let mut seed = 0x2545_F491u32;
    for i in 0..n {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let a0 = i as f32 / n as f32 * TAU;
        let a1 = (i + 1) as f32 / n as f32 * TAU;
        let hue = (0.76 - i as f32 / n as f32).rem_euclid(1.0);
        let color = hsv(hue, 0.72, if pal.dark { 0.97 } else { 0.92 });
        add_segment(&mut ring, &g, &Seg { id: 0, kind: SegKind::Node, depth: 1, a0, a1, size: 0, hidden: false }, color);
        let len = 0.25 + (seed >> 8 & 0xff) as f32 / 255.0 * 0.95;
        let faint = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), if pal.dark { 20 } else { 34 });
        let ray = Geo { center, r0: g.r0 + g.ring + 3.0, ring: radius * 0.3 * len };
        let wide = (seed >> 20).is_multiple_of(3);
        let (b0, b1) = if wide { (a0 - 0.05, a1 + 0.05) } else { (a0, a1) };
        add_segment(&mut rays, &ray, &Seg { id: 0, kind: SegKind::Node, depth: 1, a0: b0, a1: b1, size: 0, hidden: false }, faint);
    }
    p.add(Shape::mesh(rays));
    p.add(Shape::mesh(ring));
    paint_center(p, &Geo { center, r0: g.r0 - 2.0, ring: 0.0 }, pal, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Entry, F_DIR, F_HIDDEN};
    use std::path::PathBuf;

    fn e(name: &str, size: u64, flags: u8) -> Entry {
        Entry { name: name.into(), size, mtime: 0, flags }
    }

    fn tree() -> Tree {
        let mut t = Tree::new(PathBuf::from("/r"));
        let first = t.add_children(0, vec![e("a", 0, F_DIR), e("b", 300, 0), e(".h", 100, F_HIDDEN), e("tiny", 0, 0)]);
        t.add_children(first, vec![e("x", 500, 0), e("y", 100, 0)]);
        t
    }

    #[test]
    fn layout_covers_the_circle_in_order() {
        let t = tree();
        let l = build(&t, 0, 0, 0);
        let ring1: Vec<&Seg> = l.segs.iter().filter(|s| s.depth == 1).collect();
        assert_eq!(ring1.len(), 3);
        assert!(ring1[0].hidden, "hidden items come first");
        assert_eq!(ring1[1].id, 1);
        assert!((ring1.last().unwrap().a1 - TAU).abs() < 1e-3);
        for w in ring1.windows(2) {
            assert!((w[0].a1 - w[1].a0).abs() < 1e-4);
        }
        assert_eq!(l.rings, 2);
        let x = l.segs.iter().find(|s| s.id == 5).unwrap();
        assert_eq!(x.depth, 2);
        assert!(x.a0 >= ring1[1].a0 - 1e-4 && x.a1 <= ring1[1].a1 + 1e-4);
    }

    #[test]
    fn volume_space_shrinks_the_data() {
        let t = tree();
        let l = build(&t, 0, 1000, 0);
        let free = l.segs.iter().find(|s| s.kind == SegKind::Free).unwrap();
        assert!((free.a1 - TAU).abs() < 1e-3);
        assert!((free.a1 - free.a0 - TAU / 2.0).abs() < 1e-3);
    }

    #[test]
    fn hit_testing() {
        let t = tree();
        let l = build(&t, 0, 0, 0);
        let g = Geo { center: pos2(0.0, 0.0), r0: 10.0, ring: 10.0 };
        assert_eq!(hit(&l, &g, pos2(1.0, 1.0)), Hit::Center);
        assert_eq!(hit(&l, &g, pos2(100.0, 100.0)), Hit::None);
        // Just right of 12 o'clock in the first ring: the hidden segment.
        match hit(&l, &g, pos2(1.0, -15.0)) {
            Hit::Seg(i) => assert!(l.segs[i].hidden),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tiny_children_are_aggregated() {
        let mut t = Tree::new(PathBuf::from("/r"));
        let mut v = vec![e("big", 1_000_000, 0)];
        for i in 0..2000 {
            v.push(e(&format!("s{i}"), 10, 0));
        }
        t.add_children(0, v);
        let l = build(&t, 0, 0, 0);
        assert_eq!(l.segs.len(), 2);
        assert_eq!(l.segs[1].kind, SegKind::Small);
        assert_eq!(l.segs[1].size, 20_000);
    }
}
