//! DGN V8 design files: an OLE compound file whose models (Dgn-Md/#n)
//! keep their graphic elements in a compressed stream (Dgn^G/$1), and whose
//! non-model stream (Dgn^Nm/$1) holds the colour table, the levels and the
//! shared cell definitions. Element fields are little-endian; coordinates
//! are UORs.

use std::collections::HashMap;
use std::io::Read;

use super::model::{arc_cubics, paths_bounds, Path, PathBuilder, Segment, Sheet, SubPath, Text};

// ── Compound file ───────────────────────────────────────────────────────────

struct Cfb<'a> {
    b: &'a [u8],
    sector: usize,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini: Vec<u8>,
    cutoff: u32,
    entries: Vec<Entry>,
}

struct Entry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u32,
}

const END: u32 = 0xFFFF_FFFA;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

impl<'a> Cfb<'a> {
    fn open(b: &'a [u8]) -> Option<Self> {
        if !b.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
            return None;
        }
        let sector = 1usize << u16::from_le_bytes([b[30], b[31]]);
        let mini_sector_shift = u16::from_le_bytes([b[32], b[33]]);
        if mini_sector_shift != 6 {
            return None;
        }
        let mut difat: Vec<u32> = (0..109).filter_map(|i| u32_at(b, 76 + i * 4)).filter(|&v| v < END).collect();
        let (mut next, mut count) = (u32_at(b, 68)?, u32_at(b, 72)?);
        while count > 0 && next < END {
            let at = (next as usize + 1) * sector;
            for i in 0..sector / 4 - 1 {
                if let Some(v) = u32_at(b, at + i * 4).filter(|&v| v < END) {
                    difat.push(v);
                }
            }
            next = u32_at(b, at + sector - 4)?;
            count -= 1;
        }
        let mut fat = Vec::new();
        for s in difat {
            let at = (s as usize + 1) * sector;
            for i in 0..sector / 4 {
                fat.push(u32_at(b, at + i * 4).unwrap_or(END));
            }
        }
        let mut cfb = Cfb { b, sector, fat, mini_fat: Vec::new(), mini: Vec::new(), cutoff: u32_at(b, 56)?, entries: Vec::new() };
        let dir = cfb.chain(u32_at(b, 48)?, None);
        for e in dir.chunks_exact(128) {
            let len = u16::from_le_bytes([e[64], e[65]]) as usize;
            let name16: Vec<u16> = e[..len.saturating_sub(2).min(64)].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            cfb.entries.push(Entry {
                name: String::from_utf16_lossy(&name16),
                kind: e[66],
                left: u32_at(e, 68)?,
                right: u32_at(e, 72)?,
                child: u32_at(e, 76)?,
                start: u32_at(e, 116)?,
                size: u32_at(e, 120)?,
            });
        }
        let root = cfb.entries.first()?;
        let (root_start, root_size) = (root.start, root.size);
        cfb.mini = cfb.chain(root_start, Some(root_size as usize));
        let mini_fat = cfb.chain(u32_at(b, 60)?, None);
        cfb.mini_fat = mini_fat.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        Some(cfb)
    }

    fn chain(&self, mut s: u32, size: Option<usize>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut guard = 0;
        while s < END && guard < 1_000_000 {
            let at = (s as usize + 1) * self.sector;
            let Some(block) = self.b.get(at..at + self.sector) else { break };
            out.extend_from_slice(block);
            s = self.fat.get(s as usize).copied().unwrap_or(END);
            guard += 1;
        }
        if let Some(n) = size {
            out.truncate(n);
        }
        out
    }

    fn stream(&self, e: &Entry) -> Vec<u8> {
        if e.size >= self.cutoff {
            return self.chain(e.start, Some(e.size as usize));
        }
        let mut out = Vec::new();
        let mut s = e.start;
        let mut guard = 0;
        while s < END && guard < 1_000_000 {
            let at = s as usize * 64;
            let Some(block) = self.mini.get(at..at + 64) else { break };
            out.extend_from_slice(block);
            s = self.mini_fat.get(s as usize).copied().unwrap_or(END);
            guard += 1;
        }
        out.truncate(e.size as usize);
        out
    }

    /// Every stream with its full path ("Dgn-Md/#000000/Dgn^G/$1").
    fn streams(&self) -> Vec<(String, &Entry)> {
        let mut out = Vec::new();
        fn walk<'b>(c: &'b Cfb, idx: u32, path: &str, out: &mut Vec<(String, &'b Entry)>, depth: usize) {
            let Some(e) = c.entries.get(idx as usize) else { return };
            if depth > 64 {
                return;
            }
            walk(c, e.left, path, out, depth + 1);
            let p = if path.is_empty() { e.name.clone() } else { format!("{path}/{}", e.name) };
            if e.kind == 2 {
                out.push((p.clone(), e));
            } else if e.kind == 1 {
                walk(c, e.child, &p, out, depth + 1);
            }
            walk(c, e.right, path, out, depth + 1);
        }
        if let Some(root) = self.entries.first() {
            walk(self, root.child, "", &mut out, 0);
        }
        out
    }
}

/// A DGN stream: raw, or zlib data after a 16-byte header.
fn inflate(data: &[u8]) -> Vec<u8> {
    for skip in [16usize, 0] {
        if let Some(z) = data.get(skip..) {
            let mut out = Vec::new();
            if flate2::read::ZlibDecoder::new(z).read_to_end(&mut out).is_ok() && !out.is_empty() {
                return out;
            }
        }
    }
    data.to_vec()
}

// ── Elements ────────────────────────────────────────────────────────────────

/// Element records of an element stream (after its 4-byte count).
fn elements(d: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 4;
    while at + 8 <= d.len() {
        let Some(words) = u32_at(d, at + 4) else { break };
        let len = 4 + words as usize * 2;
        if len < 8 {
            break;
        }
        // The stream can end inside its last element (its trailing words are
        // not stored); what is there is still read.
        let e = &d[at..(at + len).min(d.len())];
        out.push(e);
        at += len;
    }
    out
}

fn kind(e: &[u8]) -> u8 {
    e[0]
}

fn flags(e: &[u8]) -> u16 {
    u16::from_le_bytes([e[2], e[3]])
}

/// A component of a complex element, cell or shared cell definition.
fn is_component(e: &[u8]) -> bool {
    flags(e) & 0x4000 != 0
}

fn f64_at(e: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(e.get(at..at + 8)?.try_into().ok()?))
}


/// A string stored after the "ff fe 01 00" marker, when the element has one.
fn marked_string(e: &[u8], from: usize) -> Option<String> {
    let i = e.get(from..)?.windows(4).position(|w| w == [0xff, 0xfe, 0x01, 0x00])? + from + 4;
    let end = e[i..].iter().position(|&c| c == 0 || c < 0x20).map(|n| i + n).unwrap_or(e.len());
    Some(String::from_utf8_lossy(&e[i..end]).to_string())
}

struct Styles {
    colors: Vec<[u8; 3]>,
    /// Level id → colour index.
    levels: HashMap<u32, u32>,
    /// Levels the underlay turns off: their elements are left out.
    hidden: std::collections::HashSet<u32>,
    /// True colours, in the order colour words number them (1 first).
    extended: Vec<[u8; 3]>,
}

/// The true colours of the file: a zlib-packed UTF-16 record in the
/// non-model attributes stream reading
/// `<ExtendedColors><Entry Color="(r,g,b)"/>…</ExtendedColors>`.
fn extended_colors(cfb: &Cfb) -> Vec<[u8; 3]> {
    let Some(attrs) = cfb.streams().iter().find(|(p, _)| p == "Dgn^NmA/$1").map(|(_, e)| inflate(&cfb.stream(e))) else {
        return Vec::new();
    };
    for at in 0..attrs.len().saturating_sub(2) {
        // A zlib header: deflate, and a check value divisible by 31.
        let (a, b) = (attrs[at], attrs[at + 1]);
        if a & 0x0f != 8 || ((a as u16) << 8 | b as u16) % 31 != 0 {
            continue;
        }
        let mut out = Vec::new();
        if flate2::read::ZlibDecoder::new(&attrs[at..]).read_to_end(&mut out).is_err() || out.len() < 2 {
            continue;
        }
        let units: Vec<u16> = out.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16_lossy(&units);
        if !text.contains("<ExtendedColors") {
            continue;
        }
        return text
            .split("Color=\"(")
            .skip(1)
            .filter_map(|rest| {
                let v: Vec<u8> = rest.split(')').next()?.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                (v.len() == 3).then(|| [v[0], v[1], v[2]])
            })
            .collect();
    }
    Vec::new()
}

impl Styles {
    fn color(&self, e: &[u8]) -> [u8; 3] {
        // The colour word: an index into the colour table, 0xFFFFFFFF for
        // the level's colour, or true colour n (from 1) × 256 plus the index
        // of its nearest table colour, drawn in its own RGB when the file
        // lists it.
        let explicit = u32_at(e, 52).unwrap_or(u32::MAX);
        let index = if explicit == u32::MAX {
            self.levels.get(&u32_at(e, 12).unwrap_or(0)).copied().unwrap_or(0)
        } else {
            if let Some(rgb) = (explicit >> 8).checked_sub(1).and_then(|n| self.extended.get(n as usize)) {
                return *rgb;
            }
            explicit & 0xff
        };
        self.colors.get(index as usize).copied().unwrap_or([255, 255, 255])
    }
}

/// Geometry in raw UORs, placed through a transform.
#[derive(Default, Clone)]
struct Raw {
    paths: Vec<Path>,
    texts: Vec<Text>,
    instances: Vec<Instance>,
}

#[derive(Clone)]
struct Instance {
    name: String,
    m: [f64; 4],
    origin: [f64; 2],
}

/// A 3D element (flag 0x0800 of its properties word): three coordinates a
/// point, orientations as quaternions. Its plan view is drawn.
fn is_3d(e: &[u8]) -> bool {
    u32_at(e, 40).is_some_and(|v| v & 0x0800 != 0)
}

/// The in-plane x and y axes (projected onto the plan) of a 3D element's
/// orientation quaternion w, x, y, z stored at `at` (the rows of its
/// rotation matrix).
fn quat_axes(e: &[u8], at: usize) -> Option<([f64; 2], [f64; 2])> {
    let (w, x, y, z) = (f64_at(e, at)?, f64_at(e, at + 8)?, f64_at(e, at + 16)?, f64_at(e, at + 24)?);
    Some((
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z)],
    ))
}

/// An elliptical arc on axes `u` and `v` (plan projections) as cubics.
fn projected_arc(c: [f64; 2], a: f64, b: f64, u: [f64; 2], v: [f64; 2], start: f64, sweep: f64) -> Vec<Segment> {
    let map = |p: [f64; 2]| [c[0] + a * p[0] * u[0] + b * p[1] * v[0], c[1] + a * p[0] * u[1] + b * p[1] * v[1]];
    arc_cubics([0.0, 0.0], 1.0, 1.0, 0.0, start, sweep)
        .into_iter()
        .map(|[p0, p1, p2, p3]| Segment::Cubic(map(p0), map(p1), map(p2), map(p3)))
        .collect()
}

fn read_run(run: &[&[u8]], styles: &Styles) -> Raw {
    let mut raw = Raw::default();
    // A complex shape or chain collects its components into one path.
    let mut complex: Option<(usize, PathBuilder, Option<([u8; 3], f64)>, Option<[u8; 3]>, bool)> = None;
    fn push(raw: &mut Raw, complex: &mut Option<(usize, PathBuilder, Option<([u8; 3], f64)>, Option<[u8; 3]>, bool)>, path: Path) {
        if let Some((left, pb, _, _, _)) = complex.as_mut() {
            for sp in &path.subpaths {
                for seg in &sp.segments {
                    let (a, b) = match seg {
                        Segment::Line(a, b) | Segment::Cubic(a, _, _, b) => (*a, *b),
                    };
                    if pb.current() != Some(a) {
                        if pb.current().is_none() { pb.move_to(a) } else { pb.line_to(a) }
                    }
                    match seg {
                        Segment::Line(..) => pb.line_to(b),
                        Segment::Cubic(_, c1, c2, _) => pb.cubic_to(*c1, *c2, b),
                    }
                }
            }
            *left = left.saturating_sub(1);
            if *left == 0 {
                let (_, mut pb, stroke, fill, closed) = complex.take().unwrap();
                if closed {
                    pb.close();
                }
                raw.paths.push(Path { subpaths: pb.finish(), stroke, fill });
            }
        } else {
            raw.paths.push(path);
        }
    }
    for e in run {
        if e.len() < 104 || styles.hidden.contains(&u32_at(e, 12).unwrap_or(0)) {
            continue;
        }
        let color = styles.color(e);
        let stroke = Some((color, -1.0));
        let p2 = |at: usize| -> Option<[f64; 2]> { Some([f64_at(e, at)?, f64_at(e, at + 8)?]) };
        let d3 = is_3d(e);
        match kind(e) {
            12 | 14 => {
                let n = u32_at(e, 104).unwrap_or(0) as usize;
                if n > 0 {
                    let fill = (kind(e) == 14 && e.windows(4).any(|w| w == [0x07, 0x10, 0x41, 0x00])).then_some(color);
                    complex = Some((n, PathBuilder::default(), stroke, fill, kind(e) == 14));
                }
            }
            3 => {
                if let (Some(a), Some(b)) = (p2(104), p2(if d3 { 128 } else { 120 })) {
                    push(&mut raw, &mut complex, Path { subpaths: vec![SubPath { segments: vec![Segment::Line(a, b)], closed: false }], stroke, fill: None });
                }
            }
            4 | 6 | 11 => {
                let n = u32_at(e, 104).unwrap_or(0) as usize;
                let stride = if d3 { 24 } else { 16 };
                let pts: Vec<[f64; 2]> = (0..n).filter_map(|k| p2(112 + k * stride)).collect();
                if pts.len() >= 2 {
                    let mut pb = PathBuilder::default();
                    pb.move_to(pts[0]);
                    for p in &pts[1..] {
                        pb.line_to(*p);
                    }
                    if kind(e) == 6 {
                        pb.close();
                    }
                    push(&mut raw, &mut complex, Path { subpaths: pb.finish(), stroke, fill: None });
                }
            }
            15 | 16 if d3 => {
                // Axes, orientation and centre: ellipse a 104, b 112,
                // quaternion 120, centre 152; arc start 104, sweep 112, a 120,
                // b 128, quaternion 136, centre 168.
                let arc = if kind(e) == 15 {
                    (|| Some((f64_at(e, 104)?, f64_at(e, 112)?, quat_axes(e, 120)?, p2(152)?, 0.0, std::f64::consts::TAU)))()
                } else {
                    (|| Some((f64_at(e, 120)?, f64_at(e, 128)?, quat_axes(e, 136)?, p2(168)?, f64_at(e, 104)?, f64_at(e, 112)?)))()
                };
                let Some((a, b, (u, v), c, start, sweep)) = arc else { continue };
                let sweep = if sweep == 0.0 { std::f64::consts::TAU } else { sweep };
                let closed = sweep.abs() >= std::f64::consts::TAU - 1e-9;
                push(
                    &mut raw,
                    &mut complex,
                    Path { subpaths: vec![SubPath { segments: projected_arc(c, a, b, u, v, start, sweep), closed }], stroke, fill: None },
                );
            }
            15 | 16 => {
                let arc = if kind(e) == 15 {
                    (|| Some((f64_at(e, 104)?, f64_at(e, 112)?, f64_at(e, 120)?, p2(128)?, 0.0, std::f64::consts::TAU)))()
                } else {
                    (|| Some((f64_at(e, 120)?, f64_at(e, 128)?, f64_at(e, 136)?, p2(144)?, f64_at(e, 104)?, f64_at(e, 112)?)))()
                };
                let Some((a, b, rot, c, start, sweep)) = arc else { continue };
                let pieces = arc_cubics(c, a, b, rot, start, if sweep == 0.0 { std::f64::consts::TAU } else { sweep });
                let closed = sweep.abs() >= std::f64::consts::TAU - 1e-9 || sweep == 0.0;
                push(
                    &mut raw,
                    &mut complex,
                    Path { subpaths: vec![SubPath { segments: pieces.into_iter().map(|[a, b, c, d]| Segment::Cubic(a, b, c, d)).collect(), closed }], stroke, fill: None },
                );
            }
            17 if d3 => {
                // Width 112, height 120, quaternion 144, origin 176; the
                // characters follow the string marker.
                let (Some(width), Some(height), Some((u, _)), Some(origin)) = (f64_at(e, 112), f64_at(e, 120), quat_axes(e, 144), p2(176)) else {
                    continue;
                };
                let Some(text) = marked_string(e, 200) else { continue };
                let (height, width) = (height * 6.0 / 1000.0, width * 6.0 / 1000.0);
                raw.texts.push(Text {
                    text,
                    origin,
                    height,
                    width_factor: if height > 0.0 { width / height } else { 1.0 },
                    rotation: u[1].atan2(u[0]),
                    color,
                    font: "txt".to_string(),
                });
            }
            17 => {
                let (Some(width), Some(height), Some(rotation), Some(origin)) = (f64_at(e, 112), f64_at(e, 120), f64_at(e, 144), p2(152)) else {
                    continue;
                };
                // The text: a byte count (with its 4-byte marker) and the
                // characters after the marker.
                let count = u16::from_le_bytes([e[110], e[111]]) as usize;
                let Some(chars) = e.get(174..170 + count.max(4)) else { continue };
                let text = String::from_utf8_lossy(chars).trim_end_matches(char::from(0)).to_string();
                let (height, width) = (height * 6.0 / 1000.0, width * 6.0 / 1000.0);
                raw.texts.push(Text { text, origin, height, width_factor: if height > 0.0 { width / height } else { 1.0 }, rotation, color, font: "txt".to_string() });
            }
            35 => {
                let (Some(m0), Some(m1), Some(m3), Some(m4), Some(origin)) = (f64_at(e, 160), f64_at(e, 168), f64_at(e, 184), f64_at(e, 192), p2(232)) else {
                    continue;
                };
                let Some(name) = marked_string(e, 248) else { continue };
                raw.instances.push(Instance { name: name.to_ascii_uppercase(), m: [m0, m1, m3, m4], origin });
            }
            _ => {}
        }
    }
    // A complex element cut short keeps the components it has.
    if let Some((_, mut pb, stroke, fill, closed)) = complex.take() {
        if closed {
            pb.close();
        }
        raw.paths.push(Path { subpaths: pb.finish(), stroke, fill });
    }
    raw
}

type Affine = [f64; 6];

fn apply(m: &Affine, p: [f64; 2]) -> [f64; 2] {
    [m[0] * p[0] + m[1] * p[1] + m[4], m[2] * p[0] + m[3] * p[1] + m[5]]
}

fn compose(o: &Affine, i: &Affine) -> Affine {
    [
        o[0] * i[0] + o[1] * i[2],
        o[0] * i[1] + o[1] * i[3],
        o[2] * i[0] + o[3] * i[2],
        o[2] * i[1] + o[3] * i[3],
        o[0] * i[4] + o[1] * i[5] + o[4],
        o[2] * i[4] + o[3] * i[5] + o[5],
    ]
}

fn place(raw: &Raw, m: &Affine, defs: &HashMap<String, Raw>, depth: usize, out: &mut Sheet) {
    let map = |p: [f64; 2]| apply(m, p);
    for path in &raw.paths {
        out.paths.push(Path {
            subpaths: path
                .subpaths
                .iter()
                .map(|sp| SubPath {
                    segments: sp
                        .segments
                        .iter()
                        .map(|s| match s {
                            Segment::Line(a, b) => Segment::Line(map(*a), map(*b)),
                            Segment::Cubic(a, b, c, d) => Segment::Cubic(map(*a), map(*b), map(*c), map(*d)),
                        })
                        .collect(),
                    closed: sp.closed,
                })
                .collect(),
            stroke: path.stroke,
            fill: path.fill,
        });
    }
    let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
    let turn = m[2].atan2(m[0]);
    for t in &raw.texts {
        out.texts.push(Text { origin: map(t.origin), height: t.height * scale, rotation: t.rotation + turn, ..t.clone() });
    }
    if depth >= 8 {
        return;
    }
    for inst in &raw.instances {
        let Some(def) = defs.get(&inst.name) else { continue };
        let local: Affine = [inst.m[0], inst.m[1], inst.m[2], inst.m[3], inst.origin[0], inst.origin[1]];
        place(def, &compose(m, &local), defs, depth + 1, out);
    }
}

/// Units of a model header: UORs per master and per sub unit.
fn units(header: &[u8]) -> (f64, f64) {
    // Unit definitions are numerator/denominator pairs per metre: master
    // (4180/4188), sub (4196/4204), storage (4332/4340), and the UORs per
    // storage unit (4324).
    let get = |at: usize| f64_at(header, at).filter(|v| v.is_finite() && *v > 0.0);
    let (mn, md, sn, sd) = (get(4180), get(4188), get(4196), get(4204));
    let (uor, stn, std) = (get(4324), get(4332), get(4340));
    let uor_per_metre = match (uor, stn, std) {
        (Some(u), Some(n), Some(d)) => u * n / d,
        _ => 1_000_000.0,
    };
    let master = match (mn, md) {
        (Some(n), Some(d)) => uor_per_metre * d / n,
        _ => uor_per_metre,
    };
    let sub = match (sn, sd) {
        (Some(n), Some(d)) => uor_per_metre * d / n,
        _ => master / 1000.0,
    };
    (master, sub)
}


/// Models of the file: (name, graphic stream, header stream).
fn models(cfb: &Cfb) -> Vec<(String, Vec<u8>, Vec<u8>)> {
    let streams = cfb.streams();
    let mut dirs: Vec<String> = streams
        .iter()
        .filter_map(|(p, _)| p.strip_suffix("/Dgn^G/$1").map(|d| d.to_string()))
        .collect();
    dirs.sort();
    dirs.into_iter()
        .map(|dir| {
            let get = |suffix: &str| streams.iter().find(|(p, _)| *p == format!("{dir}/{suffix}")).map(|(_, e)| inflate(&cfb.stream(e))).unwrap_or_default();
            let graphics = get("Dgn^G/$1");
            let header = get("Dgn~Mh");
            let name = marked_string(&header, 0).unwrap_or_else(|| "Default".to_string());
            (name, graphics, header)
        })
        .collect()
}

pub fn model_names(bytes: &[u8]) -> Option<Vec<String>> {
    let cfb = Cfb::open(bytes)?;
    let names: Vec<String> = models(&cfb).into_iter().map(|m| m.0).collect();
    (!names.is_empty()).then_some(names)
}

/// The level table (the table entries after a header of table 1): id,
/// name and colour index. The default level (64) is listed as "0", as the
/// reference names it.
fn level_table(nm: &[&[u8]]) -> Vec<(u32, String, u32)> {
    let mut out = Vec::new();
    let mut in_levels = false;
    for e in nm {
        match kind(e) {
            96 => in_levels = u32_at(e, 12) == Some(1),
            95 if in_levels && e.len() >= 80 => {
                let (Some(id), Some(color)) = (u32_at(e, 32), u32_at(e, 72)) else { continue };
                let mut name = marked_string(e, 32).unwrap_or_default();
                if id == 64 && name.eq_ignore_ascii_case("Default") {
                    name = "0".to_string();
                }
                out.push((id, name, color));
            }
            _ => {}
        }
    }
    out
}

fn non_model(cfb: &Cfb) -> Vec<u8> {
    cfb.streams().iter().find(|(p, _)| p == "Dgn^Nm/$1").map(|(_, e)| inflate(&cfb.stream(e))).unwrap_or_default()
}

/// Level names of the file, for the underlay layers list.
pub fn layer_names(bytes: &[u8]) -> Option<Vec<String>> {
    let cfb = Cfb::open(bytes)?;
    let nm = non_model(&cfb);
    Some(level_table(&elements(&nm)).into_iter().map(|l| l.1).filter(|n| !n.is_empty()).collect())
}

pub fn model(bytes: &[u8], name: &str, hidden: &[String]) -> Option<Sheet> {
    let cfb = Cfb::open(bytes)?;
    let all = models(&cfb);
    let (_, graphics, header) = all.iter().find(|m| m.0.eq_ignore_ascii_case(name)).or_else(|| all.first())?;
    let non_model = non_model(&cfb);
    let nm = elements(&non_model);
    let colors = nm
        .iter()
        .find(|e| kind(e) == 5 && e.len() >= 37 + 768)
        .map(|e| (0..256).map(|i| [e[37 + i * 3], e[38 + i * 3], e[39 + i * 3]]).collect())
        .unwrap_or_else(|| vec![[255, 255, 255]; 256]);
    let table = level_table(&nm);
    let levels = table.iter().map(|l| (l.0, l.2)).collect();
    let hidden = table
        .iter()
        .filter(|l| hidden.iter().any(|h| h.eq_ignore_ascii_case(&l.1)))
        .map(|l| l.0)
        .collect();
    let styles = Styles { colors, levels, hidden, extended: extended_colors(&cfb) };
    // Shared cell definitions: a type-34 element and its components.
    let mut defs = HashMap::new();
    let mut i = 0;
    while i < nm.len() {
        if kind(nm[i]) == 34 && !is_component(nm[i]) {
            let name = marked_string(nm[i], 248).unwrap_or_default().to_ascii_uppercase();
            let mut j = i + 1;
            while j < nm.len() && is_component(nm[j]) {
                j += 1;
            }
            defs.insert(name, read_run(&nm[i + 1..j], &styles));
            i = j;
        } else {
            i += 1;
        }
    }
    let raw = read_run(&elements(graphics), &styles);
    let (per_unit, sub_uor) = units(header);
    let m: Affine = [1.0 / per_unit, 0.0, 0.0, 1.0 / per_unit, 0.0, 0.0];
    let mut sheet = Sheet { sub_per_master: per_unit / sub_uor, ..Default::default() };
    place(&raw, &m, &defs, 0, &mut sheet);
    sheet.rect = paths_bounds(&sheet.paths)?;
    Some(sheet)
}
