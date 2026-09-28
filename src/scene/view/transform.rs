use codec::types::{Transform, Vector3};
use glam::DVec3;

use crate::command::EntityTransform;

#[inline]
fn to_v3(v: DVec3) -> Vector3 {
    Vector3::new(v.x, v.y, v.z)
}

pub fn apply_standard_transform<T>(entity: &mut T, center: DVec3, axis: DVec3, angle_rad: f64)
where
    T: codec::Entity,
{
    let t = Transform::from_translation(to_v3(-center))
        .then(&Transform::from_rotation(to_v3(axis.normalize_or(DVec3::Z)), angle_rad))
        .then(&Transform::from_translation(to_v3(center)));
    entity.apply_transform(&t);
}

pub fn apply_standard_scale<T>(entity: &mut T, center: DVec3, factor: f64)
where
    T: codec::Entity,
{
    let s = factor;
    let t = Transform::from_scaling_with_origin(Vector3::new(s, s, s), to_v3(center));
    entity.apply_transform(&t);
}

pub fn apply_standard_entity_transform<T, F>(entity: &mut T, t: &EntityTransform, mirror: F)
where
    T: codec::Entity,
    F: FnOnce(&mut T, DVec3, DVec3),
{
    match t {
        EntityTransform::Translate(d) => entity.translate(to_v3(*d)),
        EntityTransform::Rotate { center, axis, angle_rad } => {
            apply_standard_transform(entity, *center, *axis, *angle_rad)
        }
        EntityTransform::Scale { center, factor } => apply_standard_scale(entity, *center, *factor),
        EntityTransform::Mirror {
            p1,
            p2,
            working_normal,
        } => {
            if working_normal.normalize_or(DVec3::Z).abs_diff_eq(DVec3::Z, 1e-10) {
                mirror(entity, *p1, *p2)
            } else {
                entity.apply_transform(&reflection_about_working_line(
                    *p1,
                    *p2,
                    *working_normal,
                ));
            }
        }
        EntityTransform::Affine(transform) => entity.apply_transform(transform),
    }
}

/// Reflection across the world-XY line through `p1`→`p2` as an opencadcodec
/// `Transform`, for delegating MIRROR to opencadcodec's entity-aware
/// `apply_transform` paths (which handle direction flags, stored-angle
/// conventions and bulges themselves). Degenerate line → identity.
pub fn reflection_about_xy_line(p1: DVec3, p2: DVec3) -> codec::types::Transform {
    reflection_about_working_line(p1, p2, DVec3::Z)
}

/// Reflection through the plane that contains the picked line and is
/// perpendicular to the active working plane.
pub fn reflection_about_working_line(
    p1: DVec3,
    p2: DVec3,
    working_normal: DVec3,
) -> codec::types::Transform {
    use codec::types::{Matrix4, Transform};
    let line = p2 - p1;
    let plane_normal = line
        .cross(working_normal.normalize_or(DVec3::Z))
        .normalize_or(DVec3::ZERO);
    if plane_normal.length_squared() < 1e-12 {
        return Transform::identity();
    }
    let n = plane_normal;
    let d = 2.0 * n.dot(p1);
    Transform::from_matrix(Matrix4 {
        m: [
            [1.0 - 2.0 * n.x * n.x, -2.0 * n.x * n.y, -2.0 * n.x * n.z, d * n.x],
            [-2.0 * n.y * n.x, 1.0 - 2.0 * n.y * n.y, -2.0 * n.y * n.z, d * n.y],
            [-2.0 * n.z * n.x, -2.0 * n.z * n.y, 1.0 - 2.0 * n.z * n.z, d * n.z],
            [0.0, 0.0, 0.0, 1.0],
        ],
    })
}

pub fn reflected_point(p: DVec3, p1: DVec3, p2: DVec3, working_normal: DVec3) -> DVec3 {
    let line = p2 - p1;
    let normal = line
        .cross(working_normal.normalize_or(DVec3::Z))
        .normalize_or(DVec3::ZERO);
    if normal.length_squared() < 1e-12 {
        p
    } else {
        p - 2.0 * normal.dot(p - p1) * normal
    }
}

pub fn reflect_xy_point(x: &mut f64, y: &mut f64, p1: DVec3, p2: DVec3) {
    let ax = (p2.x - p1.x) as f64;
    let ay = (p2.y - p1.y) as f64;
    let len2 = ax * ax + ay * ay;
    if len2 < 1e-12 {
        return;
    }
    let rx = *x - p1.x as f64;
    let ry = *y - p1.y as f64;
    let dot = rx * ax + ry * ay;
    let mx = 2.0 * dot * ax / len2 - rx;
    let my = 2.0 * dot * ay / len2 - ry;
    *x = p1.x as f64 + mx;
    *y = p1.y as f64 + my;
}

/// Fully transforms ellipse / elliptical arc geometry under any 3D affine transform
/// (including negative scales, reflections, negative rotations, non-uniform scales, and translation).
///
/// Returns `(new_center, new_major_axis, new_normal, new_minor_axis_ratio, new_start_param, new_end_param)`
/// where `new_major_axis` is guaranteed to be orthogonal to `new_normal`, its length is >= semi-minor axis length,
/// and `new_minor_axis_ratio <= 1.0`.
pub fn transform_ellipse_geometry(
    center: Vector3,
    major_axis: Vector3,
    normal: Vector3,
    minor_axis_ratio: f64,
    start_param: f64,
    end_param: f64,
    transform: &Transform,
) -> Option<(Vector3, Vector3, Vector3, f64, f64, f64)> {
    let norm_len = normal.length();
    if norm_len <= 1e-12 || !norm_len.is_finite() {
        return None;
    }
    let norm = normal / norm_len;
    let m = major_axis;
    let m_len = m.length();
    if m_len <= 1e-12 || !m_len.is_finite() {
        return None;
    }
    let ratio = minor_axis_ratio.clamp(1e-12, 1.0);
    // V = (N x M) * ratio
    let v = norm.cross(&m) * ratio;

    let c_prime = transform.apply(center);
    let m0 = transform.apply_rotation(m);
    let v0 = transform.apply_rotation(v);

    let m0_len_sq = m0.length_squared();
    let v0_len_sq = v0.length_squared();
    if m0_len_sq <= 1e-24 || v0_len_sq <= 1e-24 || !m0_len_sq.is_finite() || !v0_len_sq.is_finite() {
        return None;
    }

    // Extremum angle theta to find the principal axes of the transformed ellipse
    let dot = m0.dot(&v0);
    let diff = m0_len_sq - v0_len_sq;
    let theta = 0.5 * (2.0 * dot).atan2(diff);

    let (sin_t, cos_t) = theta.sin_cos();
    let m_new = m0 * cos_t + v0 * sin_t;
    let v_new = m0 * (-sin_t) + v0 * cos_t;

    let a = m_new.length();
    let b = v_new.length();
    if a <= 1e-12 || !a.is_finite() {
        return None;
    }

    let cross = m_new.cross(&v_new);
    let cross_len = cross.length();
    if cross_len <= 1e-12 || !cross_len.is_finite() {
        return None;
    }
    let n_new = cross / cross_len;
    let new_ratio = (b / a).clamp(1e-12, 1.0);

    // Sweep and parameters
    use std::f64::consts::TAU;
    let raw = end_param - start_param;
    let is_full = (raw - TAU).abs() < 1e-9
        || (raw.abs() >= TAU - 1e-9)
        || (raw.rem_euclid(TAU).abs() < 1e-9 && raw.abs() > 1e-9);

    let (new_start, new_end) = if is_full {
        (0.0, TAU)
    } else {
        let sweep = if raw <= 0.0 {
            let s = raw.rem_euclid(TAU);
            if s <= 1e-9 { TAU } else { s }
        } else if raw > TAU {
            TAU
        } else {
            raw
        };
        let s = (start_param - theta).rem_euclid(TAU);
        (s, s + sweep)
    };

    Some((c_prime, m_new, n_new, new_ratio, new_start, new_end))
}

/// DXF arbitrary-axis algorithm — returns the OCS X and Y basis vectors in WCS
/// for a given entity normal vector.
///
/// Returns (Ax, Ay) each as (x, y, z) tuples.  When normal ≈ (0,0,1) the
/// function returns the standard basis ((1,0,0), (0,1,0)) without any
/// cross-product computation.
pub fn ocs_axes(normal: (f64, f64, f64)) -> ((f64, f64, f64), (f64, f64, f64)) {
    let (nx, ny, nz) = normal;
    // Fast path: normal is effectively Z-up, OCS = WCS.
    if nx.abs() < 1e-10 && ny.abs() < 1e-10 && (nz - 1.0).abs() < 1e-10 {
        return ((1.0, 0.0, 0.0), (0.0, 1.0, 0.0));
    }
    // Arbitrary-axis algorithm (DXF spec).
    // When |Nx| < 1/64 and |Ny| < 1/64 the normal is near ±Z, so cross with
    // Wy=(0,1,0); otherwise cross with Wz=(0,0,1). The else branch MUST use Wz,
    // not Wx: Wx=(1,0,0)×N collapses to zero for an X-aligned normal (e.g.
    // N=(-1,0,0)), zeroing the OCS axes and mapping every point to the origin —
    // the resulting coincident points then send the arc tessellator into an
    // infinite loop. (#142)
    let (ax_x, ax_y, ax_z) = if nx.abs() < 1.0 / 64.0 && ny.abs() < 1.0 / 64.0 {
        // (0,1,0) × (nx,ny,nz) = (nz, 0, -nx)
        let (x, y, z) = (nz, 0.0, -nx);
        let len = (x * x + y * y + z * z).sqrt().max(1e-12);
        (x / len, y / len, z / len)
    } else {
        // (0,0,1) × (nx,ny,nz) = (-ny, nx, 0)
        let (x, y, z) = (-ny, nx, 0.0);
        let len = (x * x + y * y + z * z).sqrt().max(1e-12);
        (x / len, y / len, z / len)
    };
    // Ay = N × Ax
    let ay_x = ny * ax_z - nz * ax_y;
    let ay_y = nz * ax_x - nx * ax_z;
    let ay_z = nx * ax_y - ny * ax_x;
    ((ax_x, ax_y, ax_z), (ay_x, ay_y, ay_z))
}

/// Transform a single OCS point to WCS using the given normal vector.
#[inline]
pub fn ocs_point_to_wcs(ocs: (f64, f64, f64), normal: (f64, f64, f64)) -> (f64, f64, f64) {
    let (ax, ay) = ocs_axes(normal);
    let (ox, oy, oz) = ocs;
    let (nx, ny, nz) = normal;
    (
        ox * ax.0 + oy * ay.0 + oz * nx,
        ox * ax.1 + oy * ay.1 + oz * ny,
        ox * ax.2 + oy * ay.2 + oz * nz,
    )
}

/// Inverse of [`ocs_point_to_wcs`]: express a WCS point (or direction — the
/// map is a pure rotation, so both use the same math) in the OCS frame of
/// `normal`. Used when writing user edits made in world space (grip drags)
/// back into a planar entity's OCS-stored coordinates.
#[inline]
pub fn wcs_point_to_ocs(wcs: (f64, f64, f64), normal: (f64, f64, f64)) -> (f64, f64, f64) {
    let (ax, ay) = ocs_axes(normal);
    let (wx, wy, wz) = wcs;
    let (nx, ny, nz) = normal;
    (
        wx * ax.0 + wy * ax.1 + wz * ax.2,
        wx * ay.0 + wy * ay.1 + wz * ay.2,
        wx * nx + wy * ny + wz * nz,
    )
}

#[cfg(test)]
mod mirror_delegation_tests {
    use super::*;
    use crate::command::EntityTransform;
    use crate::entities::traits::Transformable;
    use codec::entities::hatch::{BoundaryEdge, BoundaryPath, CircularArcEdge, LineEdge};
    use codec::entities::Hatch;
    use codec::types::Vector2;

    // MIRROR on a hatch goes through reflection_about_xy_line + opencadcodec's
    // transform_hatch. Mirror a Line→Arc→Line path across a vertical line and
    // assert the arc stays endpoint-continuous, flips its direction flag, and
    // keeps its sweep magnitude (the stored-angle conventions the old
    // hand-rolled closure violated).
    #[test]
    fn hatch_mirror_keeps_arc_continuous_and_sweep() {
        let mut path = BoundaryPath::new();
        path.edges.push(BoundaryEdge::Line(LineEdge {
            start: Vector2::new(0.0, -1.0),
            end: Vector2::new(2.0, 0.0),
        }));
        path.edges.push(BoundaryEdge::CircularArc(CircularArcEdge {
            center: Vector2::new(1.0, 0.0),
            radius: 1.0,
            start_angle: 0.0,
            end_angle: std::f64::consts::PI,
            counter_clockwise: true,
        }));
        path.edges.push(BoundaryEdge::Line(LineEdge {
            start: Vector2::new(0.0, 0.0),
            end: Vector2::new(0.0, -1.0),
        }));
        let mut h = Hatch::new();
        h.paths.push(path);

        // Mirror across the vertical line x = 5.
        h.apply_transform(&EntityTransform::Mirror {
            p1: DVec3::new(5.0, 0.0, 0.0),
            p2: DVec3::new(5.0, 1.0, 0.0),
            working_normal: DVec3::Z,
        });

        let edges = &h.paths[0].edges;
        let (l1, arc) = match (&edges[0], &edges[1]) {
            (BoundaryEdge::Line(a), BoundaryEdge::CircularArc(b)) => (a, b),
            _ => panic!("edge kinds changed"),
        };
        assert!(!arc.counter_clockwise, "mirror must flip the flag");
        let sweep = arc.end_angle - arc.start_angle;
        assert!(
            (sweep - std::f64::consts::PI).abs() < 1e-9,
            "stored sweep must stay π, got {sweep}"
        );
        // Stored-angle convention: true point of a CW edge is at -θ.
        let pt = |theta: f64| {
            let a = if arc.counter_clockwise { theta } else { -theta };
            (
                arc.center.x + arc.radius * a.cos(),
                arc.center.y + arc.radius * a.sin(),
            )
        };
        let (sx, sy) = pt(arc.start_angle);
        assert!(
            (sx - l1.end.x).abs() < 1e-9 && (sy - l1.end.y).abs() < 1e-9,
            "arc start {:?} must meet previous line end {:?}",
            (sx, sy),
            (l1.end.x, l1.end.y)
        );
        // Mirror of (2,0) across x=5 is (8,0).
        assert!((sx - 8.0).abs() < 1e-9 && sy.abs() < 1e-9);
    }
}

#[cfg(test)]
mod ocs_axes_142 {
    #[test]
    fn x_normal_axes_nonzero() {
        let (ax, ay) = super::ocs_axes((-1.0, 0.0, 0.0));
        let nz = |v: (f64,f64,f64)| v.0.abs()+v.1.abs()+v.2.abs();
        assert!(nz(ax) > 0.5, "ax collapsed: {:?}", ax);
        assert!(nz(ay) > 0.5, "ay collapsed: {:?}", ay);
        let p = super::ocs_point_to_wcs((-25.0, 90.0, 0.0), (-1.0,0.0,0.0));
        assert!(p.0.abs()+p.1.abs()+p.2.abs() > 1.0, "point collapsed: {:?}", p);
    }
}

#[cfg(test)]
mod ellipse_transform_tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

    fn sample_point(center: Vector3, major: Vector3, normal: Vector3, ratio: f64, t: f64) -> Vector3 {
        let u = major;
        let v = normal.normalize().cross(&major) * ratio;
        center + u * t.cos() + v * t.sin()
    }

    #[test]
    fn negative_rotation_negative_scale_distant_origin() {
        // Original ellipse arc at distant center (7000, 5000, 0)
        let center = Vector3::new(7000.0, 5000.0, 0.0);
        let major = Vector3::new(10.0, 0.0, 0.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        let ratio = 0.5;
        let start_param = FRAC_PI_4;
        let end_param = 3.0 * FRAC_PI_4;

        // Transform: non-uniform negative scale (-1.0, -1.0, 1.0), rotation -45 deg, translation (100, 200, 0)
        let rot_rad = -FRAC_PI_4;
        let transform = Transform::from_scaling(Vector3::new(-1.0, -1.0, 1.0))
            .then(&Transform::from_rotation(Vector3::UNIT_Z, rot_rad))
            .then(&Transform::from_translation(Vector3::new(100.0, 200.0, 0.0)));

        let (c, m, n, r, s, e) = transform_ellipse_geometry(
            center,
            major,
            normal,
            ratio,
            start_param,
            end_param,
            &transform,
        )
        .expect("transformation should succeed");

        assert!(r <= 1.0 && r > 0.0, "ratio must be in (0, 1], got {r}");
        assert!(m.length() >= m.length() * r);

        // Verify that start, end, and intermediate points match exactly
        for i in 0..=10 {
            let frac = i as f64 / 10.0;
            let orig_t = start_param + frac * (end_param - start_param);
            let orig_pt = sample_point(center, major, normal, ratio, orig_t);
            let expected_pt = transform.apply(orig_pt);

            let new_t = s + frac * (e - s);
            let actual_pt = sample_point(c, m, n, r, new_t);

            let diff = (expected_pt - actual_pt).length();
            assert!(
                diff < 1e-9,
                "Point at frac {frac} differs by {diff}: expected {expected_pt:?}, got {actual_pt:?}"
            );
        }
    }

    #[test]
    fn axis_swapping_under_non_uniform_negative_scale() {
        // When scale along Y is much larger, the minor axis becomes the major axis
        let center = Vector3::new(0.0, 0.0, 0.0);
        let major = Vector3::new(10.0, 0.0, 0.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        let ratio = 0.5; // initial minor length is 5.0
        let start_param = 0.1;
        let end_param = 2.0;

        // Scale X by -1, Scale Y by -3: transformed major length is 10, transformed minor length is 15.
        // So axes must swap!
        let transform = Transform::from_scaling(Vector3::new(-1.0, -3.0, 1.0))
            .then(&Transform::from_rotation(Vector3::UNIT_Z, -FRAC_PI_2));

        let (c, m, n, r, s, e) = transform_ellipse_geometry(
            center,
            major,
            normal,
            ratio,
            start_param,
            end_param,
            &transform,
        )
        .expect("transformation should succeed");

        assert!(r <= 1.0, "ratio must be <= 1.0 after axis swap, got {r}");
        assert!((r - (10.0 / 15.0)).abs() < 1e-9);

        // Verify points match
        for i in 0..=10 {
            let frac = i as f64 / 10.0;
            let orig_t = start_param + frac * (end_param - start_param);
            let orig_pt = sample_point(center, major, normal, ratio, orig_t);
            let expected_pt = transform.apply(orig_pt);

            let new_t = s + frac * (e - s);
            let actual_pt = sample_point(c, m, n, r, new_t);

            let diff = (expected_pt - actual_pt).length();
            assert!(
                diff < 1e-9,
                "Point at frac {frac} differs by {diff}: expected {expected_pt:?}, got {actual_pt:?}"
            );
        }
    }

    #[test]
    fn reflection_handedness_flip_arc_preservation() {
        // Reflection across Y axis (scale X by -1, Y by 1)
        let center = Vector3::new(50.0, 60.0, 0.0);
        let major = Vector3::new(8.0, 0.0, 0.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        let ratio = 0.75;
        let start_param = 0.2;
        let end_param = 1.8;

        let transform = Transform::from_scaling(Vector3::new(-1.0, 1.0, 1.0))
            .then(&Transform::from_rotation(Vector3::UNIT_Z, -0.6));

        let (c, m, n, r, s, e) = transform_ellipse_geometry(
            center,
            major,
            normal,
            ratio,
            start_param,
            end_param,
            &transform,
        )
        .expect("transformation should succeed");

        // Normal must be inverted in Z due to reflection
        assert!(n.z < 0.0, "normal Z should be flipped, got {}", n.z);

        for i in 0..=10 {
            let frac = i as f64 / 10.0;
            let orig_t = start_param + frac * (end_param - start_param);
            let orig_pt = sample_point(center, major, normal, ratio, orig_t);
            let expected_pt = transform.apply(orig_pt);

            let new_t = s + frac * (e - s);
            let actual_pt = sample_point(c, m, n, r, new_t);

            let diff = (expected_pt - actual_pt).length();
            assert!(
                diff < 1e-9,
                "Point at frac {frac} differs by {diff}: expected {expected_pt:?}, got {actual_pt:?}"
            );
        }
    }
}
