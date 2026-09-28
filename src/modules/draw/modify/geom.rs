//! Adapters between the editing commands and `opencadkernel`'s plane geometry.
//!
//! The kernel takes points as `[f64; 2]` and hands tessellation back as
//! `[f64; 3]`. The commands here grew up passing loose `x, y` scalars and
//! feeding [`WireModel::points`](crate::scene::model::wire_model::WireModel),
//! which is `[f32; 3]`. These wrappers absorb that difference in one place so
//! the maths itself lives once, in the kernel.
//!
//! Narrowing to `f32` happens here rather than in the kernel deliberately:
//! preview wires leave `points_low` empty, so they accept the loss, while the
//! resident render path splits the `f64` into a high/low pair instead. Only a
//! caller knows which of the two it is.

use kernel::geom2d::{self, Ellipse};

/// Re-exported unchanged: these already speak in plain `f64`, so there is no
/// call-shape difference for this module to absorb.
pub use kernel::geom2d::{lerp, normalize_angle};

/// Where `angle` sits along the arc from `start` to `end`, as `0.0..=1.0`.
///
/// Unlike `kernel::geom2d::arc_parameter` which clamps any angle outside the arc to 1.0,
/// this clamps angles in the gap outside the arc to 0.0 or 1.0 depending on which endpoint
/// they are closer to. This prevents cursor jitter near `start` from snapping to 1.0.
pub fn arc_parameter(angle: f64, start: f64, end: f64) -> f64 {
    let span = kernel::geom2d::arc_span(start, end);
    if span < 1e-12 {
        return 0.0;
    }
    let norm_angle = kernel::geom2d::normalize_angle(angle);
    let norm_start = kernel::geom2d::normalize_angle(start);
    let delta = (norm_angle - norm_start).rem_euclid(std::f64::consts::TAU);
    if delta <= span {
        (delta / span).clamp(0.0, 1.0)
    } else {
        let gap = std::f64::consts::TAU - span;
        let past_end = delta - span;
        if past_end <= gap * 0.5 {
            1.0
        } else {
            0.0
        }
    }
}

/// Finds the closest parametric parameter `t` on an origin-centered ellipse
/// `(x/a)^2 + (y/b)^2 = 1` for an arbitrary query point `(u, v)` in local coordinates,
/// using David Eberly's exact orthogonal projection algorithm.
///
/// Unlike radial projection `(v/b).atan2(u/a)` which deviates by up to 20-40 degrees
/// when the query point is slightly off the ellipse perimeter (e.g. within screen pick box),
/// this finds the true orthogonal projection on the ellipse regardless of zoom level.
pub fn ellipse_closest_parameter(a: f64, b: f64, u: f64, v: f64) -> f64 {
    if a < 1e-12 || b < 1e-12 {
        return 0.0;
    }
    if (a - b).abs() < 1e-9 {
        return v.atan2(u);
    }
    if a < b {
        let t_swapped = ellipse_closest_parameter(b, a, v, u);
        return std::f64::consts::FRAC_PI_2 - t_swapped;
    }

    let sign_x = if u >= 0.0 { 1.0 } else { -1.0 };
    let sign_y = if v >= 0.0 { 1.0 } else { -1.0 };
    let u_abs = u.abs();
    let v_abs = v.abs();

    if v_abs < 1e-12 {
        let x = if u_abs < (a * a - b * b) / a {
            a * a * u_abs / (a * a - b * b)
        } else {
            a
        };
        let y = b * (1.0 - (x / a).powi(2)).max(0.0).sqrt();
        return (sign_y * y / b).atan2(sign_x * x / a);
    }

    if u_abs < 1e-12 {
        return (sign_y * 1.0).atan2(0.0);
    }

    let a2 = a * a;
    let b2 = b * b;
    let mut t0 = -b2 + b * v_abs;
    let mut t1 = -b2 + (a * u_abs).hypot(b * v_abs);
    let mut t = t0;

    for _ in 0..25 {
        let t_a = t + a2;
        let t_b = t + b2;
        if t_a.abs() < 1e-12 || t_b.abs() < 1e-12 {
            break;
        }
        let f = (a * u_abs / t_a).powi(2) + (b * v_abs / t_b).powi(2) - 1.0;
        if f.abs() < 1e-12 {
            break;
        }
        let df = -2.0 * (a2 * u_abs * u_abs / (t_a.powi(3)) + b2 * v_abs * v_abs / (t_b.powi(3)));
        let t_next = t - f / df;
        if t_next > t0 && t_next < t1 {
            t = t_next;
        } else {
            if f > 0.0 {
                t0 = t;
            } else {
                t1 = t;
            }
            t = 0.5 * (t0 + t1);
        }
    }

    let x = a2 * u_abs / (t + a2);
    let y = b2 * v_abs / (t + b2);
    (sign_y * y / b).atan2(sign_x * x / a)
}

/// Preview geometry keeps the density the commands have always used.
const SEGMENTS_PER_RADIAN: f64 = geom2d::DEFAULT_SEGMENTS_PER_RADIAN;

fn narrow(points: Vec<[f64; 3]>) -> Vec<[f32; 3]> {
    points
        .into_iter()
        .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
        .collect()
}

/// Where two infinite lines cross, as `(t, u)`; `None` when parallel.
///
/// Both parameters are unbounded, so EXTEND can use a crossing that lies past
/// the end of either line.
#[allow(clippy::too_many_arguments)]
pub fn line_line(
    px: f64,
    py: f64,
    dx: f64,
    dy: f64,
    qx: f64,
    qy: f64,
    ex: f64,
    ey: f64,
) -> Option<(f64, f64)> {
    geom2d::line_line([px, py], [dx, dy], [qx, qy], [ex, ey])
}

pub fn line_points(start: [f64; 3], end: [f64; 3]) -> Vec<[f32; 3]> {
    narrow(vec![start, end])
}

/// A circular arc sampled counter-clockwise, as render vertices.
pub fn arc_points(cx: f64, cy: f64, r: f64, a0: f64, a1: f64, z: f64) -> Vec<[f32; 3]> {
    narrow(geom2d::arc([cx, cy], r, a0, a1, z, SEGMENTS_PER_RADIAN))
}

/// An elliptical arc sampled between two of the ellipse's own parameters, as
/// render vertices.
///
/// Components come back as `[x, y, z]`. The hand-rolled version this replaces
/// wrote them as `[x, z, y]`, which flattened ELLIPSE trim previews onto
/// `y = 0` and displaced them along Z.
#[allow(clippy::too_many_arguments)]
pub fn ellipse_points(
    cx: f64,
    cy: f64,
    a: f64,
    b: f64,
    nx: f64,
    ny: f64,
    t0: f64,
    t1: f64,
    z: f64,
) -> Vec<[f32; 3]> {
    let ellipse = Ellipse {
        centre: [cx, cy],
        major_radius: a,
        minor_radius: b,
        major_axis: [nx, ny],
    };
    narrow(geom2d::ellipse_arc(
        &ellipse,
        t0,
        t1,
        z,
        SEGMENTS_PER_RADIAN,
    ))
}
