//! DWF (6.x, a zip after a "(DWF V06.00)" tag, sheets as W2D streams) and
//! DWFx (an XPS package, sheets as fixed pages) underlays: sheet names, and
//! each sheet's plotted geometry mapped back to the model units it was
//! plotted from, which is how an underlay places it.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use super::model::{arc_cubics, paths_bounds, Path, PathBuilder, Segment, Sheet, SubPath};

/// The package's files, with `\` separators turned into `/`.
fn unzip(bytes: &[u8]) -> Option<HashMap<String, Vec<u8>>> {
    // A DWF 6 package is a zip after a short tag; the archive reader finds
    // the offset itself.
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut files = HashMap::new();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).ok()?;
        let name = file.name().replace('\\', "/").trim_start_matches('/').to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data).ok()?;
        files.insert(name, data);
    }
    Some(files)
}

/// One plotted sheet of the package.
struct SheetEntry {
    name: String,
    order: i64,
    /// The sheet's graphics: a W2D stream or an XPS fixed page.
    graphics: String,
    /// Paper millimetres per graphics unit and the paper offset (mm).
    paper: ([f64; 2], [f64; 2]),
    paper_height_mm: f64,
    /// DWFx: the W2X resource holding the model units transform.
    w2x: Option<String>,
}

fn attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes().find(|a| a.name() == name).map(|a| a.value())
}

fn numbers(text: &str) -> Vec<f64> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Every sheet the package plots, in plot order.
fn sheets(files: &HashMap<String, Vec<u8>>) -> Vec<SheetEntry> {
    let mut out = Vec::new();
    for (name, data) in files {
        if !name.ends_with("descriptor.xml") {
            continue;
        }
        let Ok(text) = std::str::from_utf8(data) else { continue };
        let Ok(doc) = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')) else { continue };
        let page = doc.root_element();
        if page.tag_name().name() != "Page" {
            continue;
        }
        let dir = name.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let resolve = |href: &str| -> String {
            let href = href.replace('\\', "/");
            let href = href.split('?').next().unwrap_or("").to_string();
            if let Some(abs) = href.strip_prefix('/') {
                abs.to_string()
            } else if files.contains_key(&href) {
                href
            } else {
                // Relative to the descriptor's section folder or its parent.
                let joined = format!("{dir}/{href}");
                if files.contains_key(&joined) {
                    joined
                } else {
                    let parent = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    format!("{parent}/{href}").trim_start_matches('/').to_string()
                }
            }
        };
        let mut graphics = None;
        let mut paper = ([0.0254 * 1000.0 / 1200.0; 2], [0.0; 2]);
        let mut w2x = None;
        let mut paper_height_mm = 0.0;
        for node in page.descendants() {
            match node.tag_name().name() {
                "Paper" => {
                    let scale = match attr(node, "units") {
                        Some("in") => 25.4,
                        _ => 1.0,
                    };
                    paper_height_mm = attr(node, "height").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) * scale;
                }
                "GraphicResource" if attr(node, "role") == Some("2d streaming graphics") => {
                    graphics = attr(node, "href").map(resolve);
                    let t = attr(node, "transform").map(numbers).unwrap_or_default();
                    if t.len() == 16 {
                        paper = ([t[0], t[5]], [t[12], t[13]]);
                    }
                }
                "Resource" if attr(node, "role") == Some("2d graphics extension") => {
                    w2x = attr(node, "href").map(resolve);
                }
                _ => {}
            }
        }
        let Some(graphics) = graphics else { continue };
        out.push(SheetEntry {
            name: attr(page, "name").unwrap_or("").to_string(),
            order: attr(page, "plotOrder").and_then(|v| v.parse().ok()).unwrap_or(0),
            graphics,
            paper,
            paper_height_mm,
            w2x,
        });
    }
    out.sort_by(|a, b| a.order.cmp(&b.order).then(a.name.cmp(&b.name)));
    out
}

/// Names of the package's sheets, in plot order.
pub fn sheet_names(bytes: &[u8]) -> Option<Vec<String>> {
    let files = unzip(bytes)?;
    let names: Vec<String> = sheets(&files).into_iter().map(|s| s.name).collect();
    (!names.is_empty()).then_some(names)
}

/// Graphics units → model units: model = (graphics − offset) / scale.
#[derive(Clone, Copy, Debug)]
struct Units {
    scale: [f64; 2],
    offset: [f64; 2],
}

impl Units {
    fn identity() -> Self {
        Self { scale: [1.0, 1.0], offset: [0.0, 0.0] }
    }
    fn model(&self, p: [f64; 2]) -> [f64; 2] {
        [(p[0] - self.offset[0]) / self.scale[0], (p[1] - self.offset[1]) / self.scale[1]]
    }
    fn length(&self, v: f64) -> f64 {
        v / self.scale[0].abs().max(1e-12)
    }
    /// A 4×4 matrix written row by row, translation in the last row.
    fn from_matrix(m: &[f64]) -> Option<Self> {
        (m.len() == 16 && m[0] != 0.0 && m[5] != 0.0)
            .then(|| Self { scale: [m[0], m[5]], offset: [m[12], m[13]] })
    }
}

/// The named sheet's geometry in model units.
pub fn sheet(bytes: &[u8], name: &str) -> Option<Sheet> {
    let files = unzip(bytes)?;
    let entries = sheets(&files);
    let entry = entries
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .or_else(|| (name.is_empty()).then(|| entries.first()).flatten())?;
    let data = files.get(&entry.graphics)?;
    let (paths, view) = if entry.graphics.to_ascii_lowercase().ends_with(".w2d") {
        w2d::read(data)?
    } else {
        let w2x = entry.w2x.as_ref().and_then(|p| files.get(p));
        xps::read(data, w2x.map(|v| v.as_slice()), entry)?
    };
    let rect = view.or_else(|| paths_bounds(&paths))?;
    Some(Sheet { rect, paths, texts: Vec::new(), sub_per_master: 1.0 })
}

/// The default colour map of a W2D stream: black, a 6×6×6 colour cube from
/// index 10 (red 180 steps, green 30, blue 5), greys after it.
fn default_color(index: u8) -> [u8; 3] {
    let i = index as usize;
    const LEVELS: [u8; 6] = [0, 51, 102, 153, 204, 255];
    // ponytail: only the cube is confirmed against plotted files; the entries
    // around it are a grey ramp until a file uses them.
    match i {
        10..=225 => {
            let c = i - 10;
            [LEVELS[c / 36], LEVELS[(c / 6) % 6], LEVELS[c % 6]]
        }
        0 => [0, 0, 0],
        1..=9 => {
            let g = (i * 255 / 10) as u8;
            [g, g, g]
        }
        _ => {
            let g = ((i - 226) * 255 / 29) as u8;
            [g, g, g]
        }
    }
}

mod w2d {
    use super::*;

    struct Reader<'a> {
        b: &'a [u8],
        i: usize,
    }

    impl<'a> Reader<'a> {
        fn u8(&mut self) -> Option<u8> {
            let v = *self.b.get(self.i)?;
            self.i += 1;
            Some(v)
        }
        fn i16(&mut self) -> Option<i16> {
            let v = i16::from_le_bytes(self.b.get(self.i..self.i + 2)?.try_into().ok()?);
            self.i += 2;
            Some(v)
        }
        fn u16(&mut self) -> Option<u16> {
            let v = u16::from_le_bytes(self.b.get(self.i..self.i + 2)?.try_into().ok()?);
            self.i += 2;
            Some(v)
        }
        fn i32(&mut self) -> Option<i32> {
            let v = i32::from_le_bytes(self.b.get(self.i..self.i + 4)?.try_into().ok()?);
            self.i += 4;
            Some(v)
        }
        fn count(&mut self) -> Option<usize> {
            match self.u8()? {
                0 => Some(self.u16()? as usize + 256),
                n => Some(n as usize),
            }
        }
        /// An extended ASCII opcode "(Name ...)": its name and body.
        fn extended(&mut self) -> Option<(String, &'a [u8])> {
            let start = self.i;
            let mut depth = 0i32;
            let mut quote = false;
            while self.i < self.b.len() {
                let c = self.b[self.i];
                self.i += 1;
                match c {
                    b'\'' => quote = !quote,
                    b'(' if !quote => depth += 1,
                    b')' if !quote => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = &self.b[start + 1..self.i.saturating_sub(1)];
            let name_len = body.iter().position(|c| c.is_ascii_whitespace() || *c == b'(').unwrap_or(body.len());
            Some((String::from_utf8_lossy(&body[..name_len]).to_string(), &body[name_len..]))
        }
    }

    /// "((a b c d)(e f g h)(i j k l)(m n o p))" inside a Units opcode.
    fn units_of(body: &[u8]) -> Option<Units> {
        let text = String::from_utf8_lossy(body);
        let text = text.split_once("((").map(|(_, rest)| rest).unwrap_or(&text);
        let values: Vec<f64> = text
            .split(|c: char| c == '(' || c == ')' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        Units::from_matrix(&values[..values.len().min(16)])
    }

    /// Integer points of "(View x,y x,y)" or a Contour's points.
    fn points_of(body: &[u8]) -> Vec<[f64; 2]> {
        let text = String::from_utf8_lossy(body);
        text.split_whitespace()
            .filter_map(|tok| {
                let (x, y) = tok.split_once(',')?;
                Some([x.trim_matches(')').parse().ok()?, y.trim_matches(')').parse().ok()?])
            })
            .collect()
    }

    fn rect_of(points: &[[f64; 2]]) -> Option<[f64; 4]> {
        let mut r = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in points {
            r = [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])];
        }
        (r[0] < r[2] && r[1] < r[3]).then_some(r)
    }

    /// The stream's geometry in model units, and its plotted view.
    pub fn read(b: &[u8]) -> Option<(Vec<Path>, Option<[f64; 4]>)> {
        let mut r = Reader { b, i: 0 };
        let mut units = Units::identity();
        let mut view: Option<[f64; 4]> = None;
        let mut color = [0u8, 0, 0];
        let mut weight = 0.0f64;
        let mut visible = true;
        let mut at = [0f64; 2];
        let mut raw: Vec<(Vec<SubPath>, [u8; 3], f64, bool)> = Vec::new();
        let rel = |at: &mut [f64; 2], dx: f64, dy: f64| {
            at[0] += dx;
            at[1] += dy;
            *at
        };
        while r.i < b.len() {
            let op = b[r.i];
            match op {
                b' ' | b'\t' | b'\r' | b'\n' => r.i += 1,
                b'(' => {
                    let (name, body) = r.extended()?;
                    match name.as_str() {
                        "Units" => units = units_of(body).unwrap_or(units),
                        "View" => view = rect_of(&points_of(body)).or(view),
                        "Viewport" if view.is_none() => {
                            view = rect_of(&points_of(body));
                            if let Some(u) = units_of(body) {
                                units = u;
                            }
                        }
                        "Color" => {
                            let v = super::numbers(&String::from_utf8_lossy(body));
                            if v.len() >= 3 {
                                color = [v[0] as u8, v[1] as u8, v[2] as u8];
                            }
                        }
                        _ => {}
                    }
                }
                b'{' => {
                    r.i += 1;
                    let size = r.i32()?;
                    r.i = r.i.checked_add(size.max(0) as usize)?;
                }
                b'V' => {
                    visible = true;
                    r.i += 1;
                }
                b'v' => {
                    visible = false;
                    r.i += 1;
                }
                b'c' => {
                    r.i += 1;
                    color = default_color(r.u8()?);
                }
                0x03 => {
                    r.i += 1;
                    let (cr, cg, cb, _ca) = (r.u8()?, r.u8()?, r.u8()?, r.u8()?);
                    color = [cr, cg, cb];
                }
                0x17 => {
                    r.i += 1;
                    weight = r.i32()? as f64;
                }
                0x0C | b'l' => {
                    r.i += 1;
                    let wide = op == b'l';
                    let mut pb = PathBuilder::default();
                    for k in 0..2 {
                        let (dx, dy) = if wide { (r.i32()? as f64, r.i32()? as f64) } else { (r.i16()? as f64, r.i16()? as f64) };
                        let p = rel(&mut at, dx, dy);
                        if k == 0 { pb.move_to(p) } else { pb.line_to(p) }
                    }
                    raw.push((pb.finish(), color, weight, visible));
                }
                0x10 | b'p' => {
                    r.i += 1;
                    let wide = op == b'p';
                    let n = r.count()?;
                    let mut pb = PathBuilder::default();
                    for k in 0..n {
                        let (dx, dy) = if wide { (r.i32()? as f64, r.i32()? as f64) } else { (r.i16()? as f64, r.i16()? as f64) };
                        let p = rel(&mut at, dx, dy);
                        if k == 0 { pb.move_to(p) } else { pb.line_to(p) }
                    }
                    raw.push((pb.finish(), color, weight, visible));
                }
                0x14 | b't' => {
                    r.i += 1;
                    let wide = op == b't';
                    let n = r.count()?;
                    let mut pts = Vec::with_capacity(n);
                    for _ in 0..n {
                        let (dx, dy) = if wide { (r.i32()? as f64, r.i32()? as f64) } else { (r.i16()? as f64, r.i16()? as f64) };
                        pts.push(rel(&mut at, dx, dy));
                    }
                    // A triangle strip: each point closes a triangle with the
                    // two before it.
                    let subpaths = pts
                        .windows(3)
                        .map(|t| SubPath {
                            segments: vec![Segment::Line(t[0], t[1]), Segment::Line(t[1], t[2]), Segment::Line(t[2], t[0])],
                            closed: true,
                        })
                        .collect();
                    raw.push((subpaths, color, -1.0, visible));
                }
                0x92 => {
                    // Circle or circular arc: relative centre, radius, start
                    // and end in 1/65536 turns (an end of 0 is a full turn).
                    r.i += 1;
                    let c = rel(&mut at, r.i32()? as f64, r.i32()? as f64);
                    let radius = r.i32()? as f64;
                    let (s, e) = (r.u16()? as f64, r.u16()? as f64);
                    let turn = std::f64::consts::TAU / 65536.0;
                    let start = s * turn;
                    let mut sweep = (e - s) * turn;
                    if e == 0.0 && s == 0.0 || sweep == 0.0 {
                        sweep = std::f64::consts::TAU;
                    } else if sweep < 0.0 {
                        sweep += std::f64::consts::TAU;
                    }
                    raw.push((arc_path(c, radius, radius, 0.0, start, sweep), color, weight, visible));
                }
                b'e' | b'E' => {
                    // Ellipse: relative centre, major and minor radius, start
                    // and end (1/65536 turns) and tilt.
                    r.i += 1;
                    let c = rel(&mut at, r.i32()? as f64, r.i32()? as f64);
                    let (major, minor) = (r.i32()? as f64, r.i32()? as f64);
                    let (s, e, tilt) = (r.u16()? as f64, r.u16()? as f64, r.u16()? as f64);
                    let turn = std::f64::consts::TAU / 65536.0;
                    let mut sweep = (e - s) * turn;
                    if sweep <= 0.0 {
                        sweep += std::f64::consts::TAU;
                    }
                    let fill = op == b'E';
                    raw.push((arc_path(c, major, minor, tilt * turn, s * turn, sweep), color, if fill { -1.0 } else { weight }, visible));
                }
                b'x' => {
                    // Text: relative position and a string (drawn as the
                    // outlines that follow it, so only the position counts).
                    r.i += 1;
                    rel(&mut at, r.i32()? as f64, r.i32()? as f64);
                    skip_string(&mut r)?;
                }
                _ => break,
            }
        }
        let paths = raw
            .into_iter()
            .filter(|(_, _, _, visible)| *visible)
            .map(|(subpaths, color, weight, _)| {
                let subpaths = subpaths.into_iter().map(|sp| map_subpath(sp, |p| units.model(p))).collect();
                if weight < 0.0 {
                    Path { subpaths, stroke: None, fill: Some(color) }
                } else {
                    Path { subpaths, stroke: Some((color, units.length(weight))), fill: None }
                }
            })
            .collect();
        let view = view.map(|v| {
            let a = units.model([v[0], v[1]]);
            let b = units.model([v[2], v[3]]);
            [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
        });
        Some((paths, view))
    }

    /// A W2D string: quoted ASCII, or a count followed by UTF-16 units.
    fn skip_string(r: &mut Reader) -> Option<()> {
        if r.b.get(r.i) == Some(&b'\'') {
            r.i += 1;
            while *r.b.get(r.i)? != b'\'' {
                r.i += 1;
            }
            r.i += 1;
        } else {
            let n = r.count()?;
            r.i += n * 2;
        }
        Some(())
    }
}

fn arc_path(c: [f64; 2], rx: f64, ry: f64, rot: f64, start: f64, sweep: f64) -> Vec<SubPath> {
    let pieces = arc_cubics(c, rx, ry, rot, start, sweep);
    let closed = (sweep.abs() - std::f64::consts::TAU).abs() < 1e-9;
    vec![SubPath { segments: pieces.into_iter().map(|[a, b, cc, d]| Segment::Cubic(a, b, cc, d)).collect(), closed }]
}

fn map_subpath(sp: SubPath, f: impl Fn([f64; 2]) -> [f64; 2]) -> SubPath {
    SubPath {
        segments: sp
            .segments
            .into_iter()
            .map(|s| match s {
                Segment::Line(a, b) => Segment::Line(f(a), f(b)),
                Segment::Cubic(a, b, c, d) => Segment::Cubic(f(a), f(b), f(c), f(d)),
            })
            .collect(),
        closed: sp.closed,
    }
}

mod xps {
    use super::*;

    /// Affine a, b, c, d, e, f: x' = a·x + c·y + e, y' = b·x + d·y + f.
    type Affine = [f64; 6];

    fn mul(outer: Affine, inner: Affine) -> Affine {
        let [a, b, c, d, e, f] = outer;
        let [a2, b2, c2, d2, e2, f2] = inner;
        [
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ]
    }

    fn apply(m: Affine, p: [f64; 2]) -> [f64; 2] {
        [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
    }

    fn parse_affine(text: &str) -> Option<Affine> {
        let v = numbers(text);
        (v.len() == 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
    }

    fn parse_color(text: &str) -> Option<[u8; 3]> {
        let hex = text.trim().strip_prefix('#')?;
        let hex = if hex.len() == 8 { &hex[2..] } else { hex };
        (hex.len() == 6).then(|| {
            let v = u32::from_str_radix(hex, 16).unwrap_or(0);
            [(v >> 16) as u8, (v >> 8) as u8, v as u8]
        })
    }

    /// The fixed page's paths in model units, and the plotted view.
    pub fn read(page: &[u8], w2x: Option<&[u8]>, entry: &SheetEntry) -> Option<(Vec<Path>, Option<[f64; 4]>)> {
        let mut units = Units::identity();
        let mut view_log = None;
        if let Some(w2x) = w2x.and_then(|b| std::str::from_utf8(b).ok()) {
            if let Ok(doc) = roxmltree::Document::parse(w2x.trim_start_matches('\u{feff}')) {
                for node in doc.descendants() {
                    match node.tag_name().name() {
                        "Units" => {
                            if let Some(u) = attr(node, "Transform").map(numbers).and_then(|m| Units::from_matrix(&m)) {
                                units = u;
                            }
                        }
                        "View" => {
                            let v = attr(node, "Area").map(numbers).unwrap_or_default();
                            if v.len() == 4 {
                                view_log = Some([v[0], v[1], v[2], v[3]]);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        let text = std::str::from_utf8(page).ok()?;
        let doc = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')).ok()?;
        // Page units (1/96 inch, y down) → graphics units: paper millimetres
        // with y up, less the paper offset, over the graphics scale.
        let (scale, offset) = entry.paper;
        let px_mm = 25.4 / 96.0;
        let to_logical = |p: [f64; 2]| {
            let x = p[0] * px_mm;
            let y = entry.paper_height_mm - p[1] * px_mm;
            [(x - offset[0]) / scale[0], (y - offset[1]) / scale[1]]
        };
        let mut paths = Vec::new();
        walk(doc.root_element(), [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], &mut |node, m| {
            let Some(data) = attr(node, "Data") else { return };
            let m = match attr(node, "RenderTransform").and_then(parse_affine) {
                Some(local) => mul(m, local),
                None => m,
            };
            let map = |p: [f64; 2]| units.model(to_logical(apply(m, p)));
            let subpaths: Vec<SubPath> = parse_data(data).into_iter().map(|sp| map_subpath(sp, map)).collect();
            if subpaths.is_empty() {
                return;
            }
            let stroke_scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt() * px_mm / scale[0] / units.scale[0].abs();
            let stroke = attr(node, "Stroke").and_then(parse_color).map(|c| {
                let t = attr(node, "StrokeThickness").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                (c, t * stroke_scale)
            });
            let fill = attr(node, "Fill").and_then(parse_color);
            if stroke.is_some() || fill.is_some() {
                paths.push(Path { subpaths, stroke, fill });
            }
        });
        let view = view_log.map(|v: [f64; 4]| {
            let a = units.model([v[0], v[1]]);
            let b = units.model([v[2], v[3]]);
            [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
        });
        Some((paths, view))
    }

    fn walk(node: roxmltree::Node, m: Affine, f: &mut impl FnMut(roxmltree::Node, Affine)) {
        for child in node.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "Canvas" => {
                    let m2 = match attr(child, "RenderTransform").and_then(parse_affine) {
                        Some(local) => mul(m, local),
                        None => m,
                    };
                    walk(child, m2, f);
                }
                "Path" => f(child, m),
                _ => {}
            }
        }
    }

    /// The path mini-language: M L H V C Q S A Z and their relative forms.
    fn parse_data(data: &str) -> Vec<SubPath> {
        let mut toks: Vec<Tok> = Vec::new();
        let bytes = data.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
                if c == 'F' {
                    // Fill rule prefix "F0"/"F1".
                    i += 2;
                    continue;
                }
                toks.push(Tok::Cmd(c));
                i += 1;
            } else if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    let d = bytes[i] as char;
                    let prev = bytes[i - 1] as char;
                    if d.is_ascii_digit() || d == '.' || d == 'e' || d == 'E' || ((d == '-' || d == '+') && (prev == 'e' || prev == 'E')) {
                        i += 1;
                    } else {
                        break;
                    }
                }
                if let Ok(v) = data[start..i].parse() {
                    toks.push(Tok::Num(v));
                }
            } else {
                i += 1;
            }
        }
        let mut pb = PathBuilder::default();
        let mut cmd = 'M';
        let mut k = 0;
        let mut last_ctrl: Option<[f64; 2]> = None;
        let num = |toks: &Vec<Tok>, k: &mut usize| -> Option<f64> {
            match toks.get(*k) {
                Some(Tok::Num(v)) => {
                    *k += 1;
                    Some(*v)
                }
                _ => None,
            }
        };
        while k < toks.len() {
            if let Tok::Cmd(c) = toks[k] {
                cmd = c;
                k += 1;
                if c == 'Z' || c == 'z' {
                    pb.close();
                    continue;
                }
            }
            let cur = pb.current().unwrap_or([0.0, 0.0]);
            let relative = cmd.is_ascii_lowercase();
            let pt = |x: f64, y: f64| if relative { [cur[0] + x, cur[1] + y] } else { [x, y] };
            match cmd.to_ascii_uppercase() {
                'M' => {
                    let (Some(x), Some(y)) = (num(&toks, &mut k), num(&toks, &mut k)) else { break };
                    pb.move_to(pt(x, y));
                    cmd = if relative { 'l' } else { 'L' };
                }
                'L' => {
                    let (Some(x), Some(y)) = (num(&toks, &mut k), num(&toks, &mut k)) else { break };
                    pb.line_to(pt(x, y));
                }
                'H' => {
                    let Some(x) = num(&toks, &mut k) else { break };
                    pb.line_to([if relative { cur[0] + x } else { x }, cur[1]]);
                }
                'V' => {
                    let Some(y) = num(&toks, &mut k) else { break };
                    pb.line_to([cur[0], if relative { cur[1] + y } else { y }]);
                }
                'C' => {
                    let v: Vec<f64> = (0..6).filter_map(|_| num(&toks, &mut k)).collect();
                    if v.len() < 6 { break }
                    let (c1, c2, p) = (pt(v[0], v[1]), pt(v[2], v[3]), pt(v[4], v[5]));
                    pb.cubic_to(c1, c2, p);
                    last_ctrl = Some(c2);
                    continue;
                }
                'S' => {
                    let v: Vec<f64> = (0..4).filter_map(|_| num(&toks, &mut k)).collect();
                    if v.len() < 4 { break }
                    let c1 = last_ctrl.map(|c| [2.0 * cur[0] - c[0], 2.0 * cur[1] - c[1]]).unwrap_or(cur);
                    let (c2, p) = (pt(v[0], v[1]), pt(v[2], v[3]));
                    pb.cubic_to(c1, c2, p);
                    last_ctrl = Some(c2);
                    continue;
                }
                'Q' => {
                    let v: Vec<f64> = (0..4).filter_map(|_| num(&toks, &mut k)).collect();
                    if v.len() < 4 { break }
                    let (q, p) = (pt(v[0], v[1]), pt(v[2], v[3]));
                    let c1 = [cur[0] + 2.0 / 3.0 * (q[0] - cur[0]), cur[1] + 2.0 / 3.0 * (q[1] - cur[1])];
                    let c2 = [p[0] + 2.0 / 3.0 * (q[0] - p[0]), p[1] + 2.0 / 3.0 * (q[1] - p[1])];
                    pb.cubic_to(c1, c2, p);
                }
                'A' => {
                    let v: Vec<f64> = (0..7).filter_map(|_| num(&toks, &mut k)).collect();
                    if v.len() < 7 { break }
                    let end = pt(v[5], v[6]);
                    for [_, c1, c2, p] in svg_arc(cur, v[0], v[1], v[2].to_radians(), v[3] != 0.0, v[4] != 0.0, end) {
                        pb.cubic_to(c1, c2, p);
                    }
                }
                _ => k += 1,
            }
            last_ctrl = None;
        }
        pb.finish()
    }

    enum Tok {
        Cmd(char),
        Num(f64),
    }

    /// An SVG/XPS endpoint arc as cubic pieces.
    fn svg_arc(p0: [f64; 2], rx: f64, ry: f64, phi: f64, large: bool, sweep: bool, p1: [f64; 2]) -> Vec<[[f64; 2]; 4]> {
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx == 0.0 || ry == 0.0 || p0 == p1 {
            return vec![[p0, p0, p1, p1]];
        }
        let (c, s) = (phi.cos(), phi.sin());
        let dx = (p0[0] - p1[0]) / 2.0;
        let dy = (p0[1] - p1[1]) / 2.0;
        let x1 = c * dx + s * dy;
        let y1 = -s * dx + c * dy;
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
        if lambda > 1.0 {
            rx *= lambda.sqrt();
            ry *= lambda.sqrt();
        }
        let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
        let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
        let mut coef = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
        if large == sweep {
            coef = -coef;
        }
        let cx1 = coef * rx * y1 / ry;
        let cy1 = -coef * ry * x1 / rx;
        let cx = c * cx1 - s * cy1 + (p0[0] + p1[0]) / 2.0;
        let cy = s * cx1 + c * cy1 + (p0[1] + p1[1]) / 2.0;
        let ang = |ux: f64, uy: f64, vx: f64, vy: f64| {
            let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
            a
        };
        let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
        let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
        if !sweep && dt > 0.0 {
            dt -= std::f64::consts::TAU;
        } else if sweep && dt < 0.0 {
            dt += std::f64::consts::TAU;
        }
        let mut pieces = arc_cubics([cx, cy], rx, ry, phi, t1, dt);
        // Pin the ends so the path joins exactly.
        if let Some(first) = pieces.first_mut() {
            first[0] = p0;
        }
        if let Some(last) = pieces.last_mut() {
            last[3] = p1;
        }
        pieces
    }
}
