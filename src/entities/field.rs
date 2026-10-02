//! Host-side glue for dynamic text field evaluation.
//!
//! The field language (DIESEL + AcVar), the field structure/linkage, and all
//! date math live in the reader library ([`codec::fields`]). OCS only
//! supplies the **environment** — the current clock, OS login and environment
//! variables — through [`FieldContext`]. A DWG library must stay deterministic
//! and platform-neutral, so the one genuinely system-specific bit (reading the
//! clock / OS user) lives here in the app instead.

use codec::fields::FieldContext;
use codec::types::Handle;
use codec::CadDocument;

/// OCS's environment provider for field evaluation. With the document at
/// hand it also answers the system variables the engine asks the host for.
struct OcsFieldContext<'a>(Option<&'a CadDocument>);

impl FieldContext for OcsFieldContext<'_> {
    fn now_julian(&self) -> f64 {
        // Unix epoch is Julian Day 2440587.5.
        epoch_secs() as f64 / 86_400.0 + 2_440_587.5
    }

    fn login(&self) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            for var in ["USER", "LOGNAME", "USERNAME"] {
                if let Ok(v) = std::env::var(var) {
                    if !v.is_empty() {
                        return Some(v);
                    }
                }
            }
        }
        None
    }

    fn getvar(&self, name: &str) -> Option<String> {
        self.0.and_then(|document| sysvar(document, name))
    }

    fn file_size(&self) -> Option<u64> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.0?.source_path.as_deref()?;
            return std::fs::metadata(path).ok().map(|m| m.len());
        }
        #[cfg(target_arch = "wasm32")]
        None
    }

    fn date_locale(&self) -> codec::fields::DateLocale {
        os_date_locale()
    }

    fn getenv(&self, name: &str) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            return std::env::var(name).ok().filter(|v| !v.is_empty());
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = name;
            None
        }
    }
}

/// Re-evaluate the field hosted by entity `host` (usually an MTEXT), or `None`
/// to keep the cached text. Thin wrapper over the library engine.
pub fn resolve(document: &CadDocument, host: Handle) -> Option<String> {
    codec::fields::resolve(document, host, &OcsFieldContext(Some(document)))
}

pub fn resolve_handle(
    document: &CadDocument,
    field: Handle,
    host: Handle,
) -> Option<String> {
    codec::fields::resolve_handle(document, field, host, &OcsFieldContext(Some(document)))
}

/// Evaluate one field code (`\AcVar Date \f "yyyy-MM-dd"`, or the `%<…>%`
/// form) for a preview; `None` when it cannot be evaluated.
pub fn evaluate(
    document: &CadDocument,
    code: &str,
    objects: &[Handle],
    host: Option<Handle>,
) -> Option<String> {
    codec::fields::evaluate_code(document, code, objects, host, &OcsFieldContext(Some(document)))
}

/// The system variables the Field dialog lists, in its order (lower case).
pub const SYSVARS: &[&str] = &[
    "aunits", "auprec", "celtscale", "chamfera", "chamferb", "chamferc", "chamferd",
    "clayer", "dimscale", "dimtxt", "dwgname", "dwgprefix", "elevation", "filletrad",
    "insbase", "insunits", "limmax", "limmin", "ltscale", "lunits", "luprec",
    "measurement", "mirrtext", "pdmode", "pdsize", "textsize", "textstyle", "thickness",
    "useri1", "useri2", "useri3", "useri4", "useri5", "userr1", "userr2", "userr3",
    "userr4", "userr5",
];

fn num(v: f64) -> String {
    let t = format!("{v:.4}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A system variable's value as a field shows it.
pub fn sysvar(document: &CadDocument, name: &str) -> Option<String> {
    let h = &document.header;
    let path = document.source_path.as_deref().unwrap_or("");
    let (folder, file) = match path.rfind(['/', '\\']) {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "aunits" => h.angular_unit_format.to_string(),
        "auprec" => h.angular_unit_precision.to_string(),
        "celtscale" => num(h.current_entity_linetype_scale),
        "chamfera" => num(h.chamfer_distance_a),
        "chamferb" => num(h.chamfer_distance_b),
        "chamferc" => num(h.chamfer_length),
        "chamferd" => num(h.chamfer_angle),
        "clayer" => h.current_layer_name.clone(),
        "dimscale" => num(h.dim_scale),
        "dimtxt" => num(h.dim_text_height),
        "dwgname" => if file.is_empty() { "Drawing1.dwg".to_string() } else { file.to_string() },
        "dwgprefix" => folder.to_string(),
        "elevation" => num(h.elevation),
        "filletrad" => num(h.fillet_radius),
        "insbase" => {
            let p = h.model_space_insertion_base;
            format!("{},{},{}", num(p.x), num(p.y), num(p.z))
        }
        "insunits" => h.insertion_units.to_string(),
        "limmax" => format!("{},{}", num(h.model_space_limits_max.x), num(h.model_space_limits_max.y)),
        "limmin" => format!("{},{}", num(h.model_space_limits_min.x), num(h.model_space_limits_min.y)),
        "ltscale" => num(h.linetype_scale),
        "lunits" => h.linear_unit_format.to_string(),
        "luprec" => h.linear_unit_precision.to_string(),
        "measurement" => h.measurement.to_string(),
        "mirrtext" => i16::from(h.mirror_text).to_string(),
        "pdmode" => h.point_display_mode.to_string(),
        "pdsize" => num(h.point_display_size),
        "textsize" => num(h.text_height),
        "textstyle" => h.current_text_style_name.clone(),
        "thickness" => num(h.thickness),
        "useri1" => h.user_int1.to_string(),
        "useri2" => h.user_int2.to_string(),
        "useri3" => h.user_int3.to_string(),
        "useri4" => h.user_int4.to_string(),
        "useri5" => h.user_int5.to_string(),
        "userr1" => num(h.user_real1),
        "userr2" => num(h.user_real2),
        "userr3" => num(h.user_real3),
        "userr4" => num(h.user_real4),
        "userr5" => num(h.user_real5),
        _ => return None,
    })
}

/// Seconds since the Unix epoch. Native uses the system clock; wasm uses the JS
/// `Date` clock. This platform-specific read stays in the app, not the library.
fn epoch_secs() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
}

/// Attach `field` (its code and referenced objects) to `host`, showing
/// `value`; `None` removes the host's field.
pub fn set_text_field(
    document: &mut CadDocument,
    host: Handle,
    field: Option<(String, Vec<Handle>)>,
    value: &str,
) {
    match field {
        Some((code, objects)) => {
            let mut child = codec::fields::NewField::new(code, value);
            child.objects = objects;
            document.set_text_field(host, "%<\\_FldIdx 0>%", vec![child]);
        }
        None => {
            document.remove_text_field(host);
        }
    }
}

/// Month and day names and the regional date / time pictures, as date
/// fields show them: the operating system's regional settings.
fn os_date_locale() -> codec::fields::DateLocale {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Globalization::GetLocaleInfoEx;
        let info = |kind: u32| -> Option<String> {
            let mut buffer = [0u16; 128];
            // A null locale name is the user's default locale.
            let len = unsafe { GetLocaleInfoEx(std::ptr::null(), kind, buffer.as_mut_ptr(), buffer.len() as i32) };
            (len > 1).then(|| String::from_utf16_lossy(&buffer[..len as usize - 1]))
        };
        let mut locale = codec::fields::DateLocale::default();
        for k in 0..12u32 {
            if let Some(v) = info(0x38 + k) {
                locale.months[k as usize] = v; // LOCALE_SMONTHNAME1..
            }
            if let Some(v) = info(0x44 + k) {
                locale.months_abbr[k as usize] = v; // LOCALE_SABBREVMONTHNAME1..
            }
        }
        // LOCALE_SDAYNAME1 is Monday; the codec lists Sunday first.
        for k in 0..7u32 {
            let slot = ((k + 1) % 7) as usize;
            if let Some(v) = info(0x2A + k) {
                locale.days[slot] = v;
            }
            if let Some(v) = info(0x31 + k) {
                locale.days_abbr[slot] = v;
            }
        }
        if let Some(v) = info(0x28) {
            locale.am = v; // LOCALE_S1159
        }
        if let Some(v) = info(0x29) {
            locale.pm = v; // LOCALE_S2359
        }
        if let Some(v) = info(0x1F) {
            locale.short_date = v; // LOCALE_SSHORTDATE
        }
        if let Some(v) = info(0x20) {
            locale.long_date = v; // LOCALE_SLONGDATE
        }
        if let Some(v) = info(0x1003) {
            locale.long_time = v; // LOCALE_STIMEFORMAT
        }
        locale
    }
    #[cfg(not(target_os = "windows"))]
    codec::fields::DateLocale::default()
}
