//! Vector content of a DWF sheet or DGN model, in the underlay's own units
//! (the sheet's model units), and what an underlay needs from it.

/// One segment of a path.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Line([f64; 2], [f64; 2]),
    Cubic([f64; 2], [f64; 2], [f64; 2], [f64; 2]),
}

impl Segment {
    pub fn start(&self) -> [f64; 2] {
        match self {
            Segment::Line(a, _) | Segment::Cubic(a, ..) => *a,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubPath {
    pub segments: Vec<Segment>,
    pub closed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub subpaths: Vec<SubPath>,
    /// Stroke colour and width (sheet units; 0 draws one pixel wide).
    pub stroke: Option<([u8; 3], f64)>,
    pub fill: Option<[u8; 3]>,
}

/// A sheet (DWF) or model (DGN): its extent and what it draws.
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    /// min x, min y, max x, max y in sheet units.
    pub rect: [f64; 4],
    pub paths: Vec<Path>,
    pub texts: Vec<Text>,
    /// DGN: sub units per master unit (the scale a Sub conversion offers).
    pub sub_per_master: f64,
}

/// Builds paths from pen moves.
#[derive(Default)]
pub struct PathBuilder {
    pub subpaths: Vec<SubPath>,
    current: Vec<Segment>,
    start: Option<[f64; 2]>,
    at: Option<[f64; 2]>,
}

impl PathBuilder {
    pub fn move_to(&mut self, p: [f64; 2]) {
        self.flush(false);
        self.start = Some(p);
        self.at = Some(p);
    }

    pub fn line_to(&mut self, p: [f64; 2]) {
        let Some(a) = self.at else {
            self.move_to(p);
            return;
        };
        self.current.push(Segment::Line(a, p));
        self.at = Some(p);
    }

    pub fn cubic_to(&mut self, c1: [f64; 2], c2: [f64; 2], p: [f64; 2]) {
        let Some(a) = self.at else {
            self.move_to(p);
            return;
        };
        self.current.push(Segment::Cubic(a, c1, c2, p));
        self.at = Some(p);
    }

    pub fn close(&mut self) {
        if let (Some(s), Some(a)) = (self.start, self.at) {
            if s != a {
                self.current.push(Segment::Line(a, s));
            }
        }
        self.flush(true);
        self.at = self.start;
    }

    pub fn current(&self) -> Option<[f64; 2]> {
        self.at
    }

    fn flush(&mut self, closed: bool) {
        if !self.current.is_empty() {
            self.subpaths.push(SubPath { segments: std::mem::take(&mut self.current), closed });
        }
    }

    pub fn finish(mut self) -> Vec<SubPath> {
        self.flush(false);
        self.subpaths
    }
}

/// Cubic pieces of an elliptical arc: centre, radii, axis rotation and the
/// start and sweep angles (radians, counter-clockwise positive).
pub fn arc_cubics(
    center: [f64; 2],
    rx: f64,
    ry: f64,
    rotation: f64,
    start: f64,
    sweep: f64,
) -> Vec<[[f64; 2]; 4]> {
    let n = ((sweep.abs() / (std::f64::consts::FRAC_PI_2)).ceil() as usize).max(1);
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let (cr, sr) = (rotation.cos(), rotation.sin());
    let at = |x: f64, y: f64| [center[0] + x * cr - y * sr, center[1] + x * sr + y * cr];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a0 = start + step * i as f64;
        let a1 = a0 + step;
        let (c0, s0) = (a0.cos(), a0.sin());
        let (c1, s1) = (a1.cos(), a1.sin());
        let p0 = at(rx * c0, ry * s0);
        let p3 = at(rx * c1, ry * s1);
        let p1 = at(rx * (c0 - k * s0), ry * (s0 + k * c0));
        let p2 = at(rx * (c1 + k * s1), ry * (s1 - k * c1));
        out.push([p0, p1, p2, p3]);
    }
    out
}

/// Bounds of the paths, when there are any.
pub fn paths_bounds(paths: &[Path]) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut add = |p: [f64; 2]| {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    };
    for path in paths {
        for sp in &path.subpaths {
            for seg in &sp.segments {
                match seg {
                    Segment::Line(a, c) => {
                        add(*a);
                        add(*c);
                    }
                    Segment::Cubic(a, c1, c2, c) => {
                        add(*a);
                        add(*c1);
                        add(*c2);
                        add(*c);
                    }
                }
            }
        }
    }
    (b[0] <= b[2]).then_some(b)
}

/// A line of text the host draws with its own font.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub text: String,
    /// Baseline start, sheet units.
    pub origin: [f64; 2],
    pub height: f64,
    /// Width of a character relative to its height.
    pub width_factor: f64,
    pub rotation: f64,
    pub color: [u8; 3],
}

/// Points of a B-spline curve: its order, control points and knot vector
/// (clamped: `order` equal knots at each end).
pub fn bspline_points(order: usize, poles: &[[f64; 2]], knots: &[f64], samples: usize) -> Vec<[f64; 2]> {
    let n = poles.len();
    if order < 2 || n < order || knots.len() != n + order {
        return poles.to_vec();
    }
    let (t0, t1) = (knots[order - 1], knots[n]);
    let mut out = Vec::with_capacity(samples + 1);
    for s in 0..=samples {
        let t = t0 + (t1 - t0) * s as f64 / samples as f64;
        // de Boor
        let mut k = order - 1;
        while k < n - 1 && t >= knots[k + 1] {
            k += 1;
        }
        let mut d: Vec<[f64; 2]> = (0..order).map(|j| poles[j + k + 1 - order]).collect();
        for r in 1..order {
            for j in (r..order).rev() {
                let i = j + k + 1 - order;
                let den = knots[i + order - r] - knots[i];
                let a = if den.abs() < 1e-15 { 0.0 } else { (t - knots[i]) / den };
                d[j] = [d[j - 1][0] * (1.0 - a) + d[j][0] * a, d[j - 1][1] * (1.0 - a) + d[j][1] * a];
            }
        }
        out.push(d[order - 1]);
    }
    out
}
