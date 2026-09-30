// POINTCLOUDSTYLIZE — colour the chosen point clouds by one stylization.
//
//   Select point cloud objects:
//   Enter a stylization option [RGB/Object color/Intensity/Elevation/Normal/Classification] <RGB>:
//   1 point cloud(s) stylized
//
// The option offered is the last one chosen. The host applies it (one undo
// step) and reports clouds that lack the data.

use std::sync::Mutex;

use codec::types::Handle;

use crate::command::{CadCommand, CmdOption, CmdResult, InputKind};

/// The options with their stored stylization values.
pub const OPTIONS: [(&str, &str, i16); 6] = [
    ("RGB", "R", 1),
    ("Object color", "O", 2),
    ("Intensity", "I", 5),
    ("Elevation", "E", 4),
    ("Normal", "N", 3),
    ("Classification", "C", 6),
];

/// The option last chosen (RGB to begin with).
static LAST: Mutex<usize> = Mutex::new(0);

pub struct PointCloudStylizeCommand {
    handles: Vec<Handle>,
}

impl PointCloudStylizeCommand {
    pub fn new(handles: Vec<Handle>) -> Self {
        Self { handles }
    }
}

impl CadCommand for PointCloudStylizeCommand {
    fn name(&self) -> &'static str {
        "POINTCLOUDSTYLIZE"
    }

    fn prompt(&self) -> String {
        let last = LAST.lock().map(|v| *v).unwrap_or(0);
        format!(
            "Enter a stylization option [RGB/Object color/Intensity/Elevation/Normal/Classification] <{}>:",
            OPTIONS[last].0
        )
    }

    fn options(&self) -> Vec<CmdOption> {
        OPTIONS.iter().map(|(name, key, _)| CmdOption::new(name, key)).collect()
    }

    fn input_kind(&self) -> InputKind {
        InputKind::SingleToken
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let text = text.trim().to_ascii_uppercase();
        let chosen = if text.is_empty() {
            LAST.lock().map(|v| *v).unwrap_or(0)
        } else {
            match OPTIONS.iter().position(|(name, key, _)| {
                text == *key || (text.len() >= key.len() && name.to_ascii_uppercase().starts_with(&text))
            }) {
                Some(at) => at,
                None => return Some(CmdResult::ReportError("Invalid option keyword.".to_string())),
            }
        };
        if let Ok(mut last) = LAST.lock() {
            *last = chosen;
        }
        let handles: Vec<String> = self.handles.iter().map(|h| format!("{:X}", h.value())).collect();
        Some(CmdResult::Dispatch(format!("_PCSTYLIZEAPPLY {} {}", OPTIONS[chosen].2, handles.join(" "))))
    }

    fn on_point(&mut self, _pt: glam::DVec3) -> CmdResult {
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        self.on_text_input("").unwrap_or(CmdResult::Cancel)
    }
}

inventory::submit!(crate::command::CommandRegistration { names: &["POINTCLOUDSTYLIZE"] });
