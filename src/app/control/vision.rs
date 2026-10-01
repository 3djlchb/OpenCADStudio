use crate::app::OpenCADStudio;
use serde_json::{json, Value};

/// 5x7 bitmapped font glyphs for digits '0' through '9'
const DIGITS_5X7: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111], // 2
    [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

/// Draw a Set-of-Marks badge at `(center_x, center_y)` onto `image`.
pub(crate) fn draw_som_badge(
    image: &mut image::RgbaImage,
    center_x: i32,
    center_y: i32,
    tag: usize,
    selected: bool,
) {
    let tag_str = tag.to_string();
    let num_digits = tag_str.len();
    if num_digits == 0 {
        return;
    }
    let text_w = num_digits * 5 + (num_digits - 1) * 1;
    let badge_w = (text_w + 8) as i32;
    let badge_h = 13i32;

    let img_w = image.width() as i32;
    let img_h = image.height() as i32;

    // Anchor badge slightly above the center anchor point if possible
    let x0 = (center_x - badge_w / 2).clamp(1, (img_w - badge_w - 1).max(1));
    let y0 = (center_y - badge_h - 2).clamp(1, (img_h - badge_h - 1).max(1));

    let (bg, border, text_col) = if selected {
        (
            image::Rgba([0, 230, 255, 245]), // Electric cyan
            image::Rgba([0, 60, 80, 255]),
            image::Rgba([0, 0, 0, 255]),
        )
    } else {
        (
            image::Rgba([255, 220, 0, 245]), // Vibrant yellow
            image::Rgba([40, 30, 0, 255]),
            image::Rgba([0, 0, 0, 255]),
        )
    };

    // Fill badge background and border
    for y in y0..(y0 + badge_h) {
        for x in x0..(x0 + badge_w) {
            if x < 0 || x >= img_w || y < 0 || y >= img_h {
                continue;
            }
            let is_border = x == x0 || x == x0 + badge_w - 1 || y == y0 || y == y0 + badge_h - 1;
            let col = if is_border { border } else { bg };
            image.put_pixel(x as u32, y as u32, col);
        }
    }

    // Draw digits
    let mut cur_x = x0 + 4;
    let cur_y = y0 + 3;
    for ch in tag_str.chars() {
        if let Some(digit) = ch.to_digit(10) {
            let d = digit as usize;
            if d < 10 {
                for r in 0..7 {
                    let row_bits = DIGITS_5X7[d][r];
                    for c in 0..5 {
                        if (row_bits & (1 << (4 - c))) != 0 {
                            let px = cur_x + c as i32;
                            let py = cur_y + r as i32;
                            if px >= 0 && px < img_w && py >= 0 && py < img_h {
                                image.put_pixel(px as u32, py as u32, text_col);
                            }
                        }
                    }
                }
            }
        }
        cur_x += 6;
    }

    // Small 3x3 indicator dot at the center anchor
    let dot_col = if selected {
        image::Rgba([0, 230, 255, 255])
    } else {
        image::Rgba([255, 60, 0, 255])
    };
    for dy in -1..=1 {
        for dx in -1..=1 {
            let px = center_x + dx;
            let py = center_y + dy;
            if px >= 0 && px < img_w && py >= 0 && py < img_h {
                image.put_pixel(px as u32, py as u32, dot_col);
            }
        }
    }
}

pub(crate) struct VisionGrounding {
    pub viewport_world_bounds: Value,
    pub camera: Value,
    pub visible_entities: Vec<Value>,
}

pub(crate) fn compute_grounding(
    app: &OpenCADStudio,
    vp_logical_width: f32,
    vp_logical_height: f32,
    image_width: u32,
    image_height: u32,
) -> VisionGrounding {
    let tab = &app.tabs[app.active_tab];
    let c = tab.scene.camera.borrow();
    let vp_rect = iced::Rectangle {
        x: 0.0,
        y: 0.0,
        width: vp_logical_width.max(1.0),
        height: vp_logical_height.max(1.0),
    };

    let p_tl = c.pick_on_target_plane(iced::Point::new(0.0, 0.0), vp_rect);
    let p_tr = c.pick_on_target_plane(iced::Point::new(vp_rect.width, 0.0), vp_rect);
    let p_bl = c.pick_on_target_plane(iced::Point::new(0.0, vp_rect.height), vp_rect);
    let p_br = c.pick_on_target_plane(iced::Point::new(vp_rect.width, vp_rect.height), vp_rect);

    let min_x = p_tl.x.min(p_tr.x).min(p_bl.x).min(p_br.x);
    let max_x = p_tl.x.max(p_tr.x).max(p_bl.x).max(p_br.x);
    let min_y = p_tl.y.min(p_tr.y).min(p_bl.y).min(p_br.y);
    let max_y = p_tl.y.max(p_tr.y).max(p_bl.y).max(p_br.y);

    let viewport_world_bounds = json!({
        "min": [min_x, min_y],
        "max": [max_x, max_y]
    });

    let eye = c.eye();
    let target = c.target;
    let camera_info = json!({
        "eye": [eye.x, eye.y, eye.z],
        "target": [target.x, target.y, target.z],
        "distance": c.distance,
        "ortho_size": c.ortho_size(),
    });

    let img_w = image_width as f32;
    let img_h = image_height as f32;

    struct Candidate {
        handle: codec::Handle,
        entity_type: &'static str,
        layer: String,
        screen_pixel: [i32; 2],
        screen_bounds: [i32; 4],
        world_center: [f64; 3],
        selected: bool,
        area: f32,
    }

    let selected_set: std::collections::HashSet<codec::Handle> =
        tab.scene.selected_handles_in_order().into_iter().collect();

    let mut candidates = Vec::new();

    for entity in tab.scene.document.entities() {
        let handle = entity.common().handle;
        let (b_min, b_max) = crate::scene::convert::tess::entity_bounds_in(&tab.scene.document, entity);
        let min_dvec = glam::DVec3::from(b_min);
        let max_dvec = glam::DVec3::from(b_max);
        if !min_dvec.is_finite() || !max_dvec.is_finite() {
            continue;
        }
        let center = (min_dvec + max_dvec) * 0.5;
        if let Some(screen_pt) = c.project(center, vp_rect) {
            if screen_pt.x >= 0.0
                && screen_pt.x <= vp_rect.width
                && screen_pt.y >= 0.0
                && screen_pt.y <= vp_rect.height
            {
                let px = (screen_pt.x / vp_rect.width * img_w).round() as i32;
                let py = (screen_pt.y / vp_rect.height * img_h).round() as i32;

                let mut s_min_x = px;
                let mut s_max_x = px;
                let mut s_min_y = py;
                let mut s_max_y = py;

                if let (Some(p0), Some(p1)) =
                    (c.project(min_dvec, vp_rect), c.project(max_dvec, vp_rect))
                {
                    let x0 = (p0.x / vp_rect.width * img_w).round() as i32;
                    let y0 = (p0.y / vp_rect.height * img_h).round() as i32;
                    let x1 = (p1.x / vp_rect.width * img_w).round() as i32;
                    let y1 = (p1.y / vp_rect.height * img_h).round() as i32;
                    s_min_x = x0.min(x1);
                    s_max_x = x0.max(x1);
                    s_min_y = y0.min(y1);
                    s_max_y = y0.max(y1);
                }

                let area = ((s_max_x - s_min_x).abs() * (s_max_y - s_min_y).abs()) as f32;
                let selected = selected_set.contains(&handle);
                let layer = entity.common().layer.clone();

                candidates.push(Candidate {
                    handle,
                    entity_type: crate::entities::names::ui_name(entity),
                    layer,
                    screen_pixel: [px, py],
                    screen_bounds: [s_min_x, s_min_y, s_max_x, s_max_y],
                    world_center: [center.x, center.y, center.z],
                    selected,
                    area,
                });
            }
        }
    }

    // Selected entities prioritized, then larger visual area first
    candidates.sort_by(|a, b| {
        b.selected
            .cmp(&a.selected)
            .then_with(|| b.area.partial_cmp(&a.area).unwrap_or(std::cmp::Ordering::Equal))
    });

    candidates.truncate(64);

    let visible_entities = candidates
        .into_iter()
        .enumerate()
        .map(|(idx, cand)| {
            json!({
                "tag": idx + 1,
                "handle": format!("{:X}", cand.handle.value()),
                "type": cand.entity_type,
                "layer": cand.layer,
                "screen_pixel": cand.screen_pixel,
                "screen_bounds": cand.screen_bounds,
                "world_center": cand.world_center,
                "selected": cand.selected
            })
        })
        .collect();

    VisionGrounding {
        viewport_world_bounds,
        camera: camera_info,
        visible_entities,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_draw_som_badge_renders_without_panics() {
        let mut img = image::RgbaImage::from_pixel(120, 100, image::Rgba([40, 40, 40, 255]));

        // Render normal badge
        draw_som_badge(&mut img, 50, 50, 1, false);

        // Center dot should be drawn
        let dot = img.get_pixel(50, 50);
        assert_eq!(*dot, image::Rgba([255, 60, 0, 255]));

        // Render selected badge with multi-digit tag near edge (clamped)
        draw_som_badge(&mut img, 2, 2, 42, true);
        let selected_dot = img.get_pixel(2, 2);
        assert_eq!(*selected_dot, image::Rgba([0, 230, 255, 255]));

        // Right edge clamp
        draw_som_badge(&mut img, 118, 98, 99, false);
    }

    #[test]
    fn test_compute_grounding_identifies_visible_entities() {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        app.automation_op(r#"{"op":"run","cmd":"CIRCLE 0,0 5"}"#);
        app.automation_op(r#"{"op":"run","cmd":"LINE -10,-10 10,10"}"#);

        // Set test canvas viewport size
        app.tabs[0].scene.selection.borrow_mut().vp_size = (800.0, 600.0);
        app.tabs[0].scene.fit_all();

        let grounding = compute_grounding(&app, 800.0, 600.0, 800, 600);

        // Viewport world bounds should be valid
        assert!(grounding.viewport_world_bounds["min"].is_array());
        assert!(grounding.viewport_world_bounds["max"].is_array());

        // Camera info should be populated
        assert!(grounding.camera["eye"].is_array());
        assert!(grounding.camera["target"].is_array());

        // Both entities should be visible and tagged
        assert_eq!(grounding.visible_entities.len(), 2);

        let first = &grounding.visible_entities[0];
        assert_eq!(first["tag"], 1);
        assert!(first["handle"].is_string());
        assert!(first["type"].is_string());
        assert!(first["screen_pixel"].is_array());
        assert!(first["screen_bounds"].is_array());
        assert!(first["world_center"].is_array());

        let second = &grounding.visible_entities[1];
        assert_eq!(second["tag"], 2);
    }
}
