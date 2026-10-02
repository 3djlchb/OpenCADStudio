//! FIELD and the Field dialog behind the ƒ buttons of the attribute
//! definition dialogs.

use crate::app::{Message, OpenCADStudio};
use crate::command::CadCommand;
use crate::modules::annotate::field_cmd::{FieldObjectPickCommand, FieldPlaceCommand};
use crate::ui::window::field_dialog::{
    fields_of, object_properties, FieldDialogMsg, FieldDialogState, FieldTarget, DATE_FORMATS,
    NAMED_TYPES,
};
use iced::Task;

impl OpenCADStudio {
    pub(super) fn dispatch_field(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            "FIELD" => {
                self.open_field_dialog(FieldTarget::NewText);
                Some(Task::none())
            }
            cmd if cmd == "_FIELD_OBJECT" || cmd.starts_with("_FIELD_OBJECT ") => {
                let handle = cmd
                    .split_whitespace()
                    .nth(1)
                    .and_then(|h| u64::from_str_radix(h, 16).ok())
                    .map(codec::Handle::new);
                if let Some(state) = self.field_dialog.as_mut() {
                    if let Some(entity) = handle.and_then(|h| self.tabs[i].scene.document.get_entity(h)) {
                        let props = object_properties(entity);
                        state.object = Some((entity.common().handle, crate::t!(crate::entities::names::ui_name(entity)).into_owned()));
                        state.object_prop = props.first().copied();
                        state.object_props = props;
                    }
                    self.active_modal = Some(crate::app::ModalKind::Field);
                    self.refresh_field_preview();
                }
                Some(Task::none())
            }
            _ => None,
        }
    }

    /// Open the Field dialog for `target`.
    pub(in crate::app) fn open_field_dialog(&mut self, target: FieldTarget) {
        let mut state = FieldDialogState::new(target);
        state.examples = DATE_FORMATS
            .iter()
            .map(|f| self.field_value(&format!("\\AcVar Date \\f \"{f}\""), &[]))
            .collect();
        self.field_dialog = Some(state);
        self.fill_named_objects();
        self.refresh_field_preview();
        self.active_modal = Some(crate::app::ModalKind::Field);
    }

    fn field_value(&self, code: &str, objects: &[codec::Handle]) -> String {
        crate::entities::field::evaluate(&self.tabs[self.active_tab].scene.document, code, objects, None)
            .unwrap_or_else(|| "----".to_string())
    }

    fn refresh_field_preview(&mut self) {
        let Some(state) = self.field_dialog.as_ref() else {
            return;
        };
        let (code, objects) = state.code();
        let preview = if state.complete() { self.field_value(&code, &objects) } else { String::new() };
        if let Some(state) = self.field_dialog.as_mut() {
            state.preview = preview;
        }
    }

    /// The names of the chosen named-object type, sorted.
    fn fill_named_objects(&mut self) {
        let Some(state) = self.field_dialog.as_mut() else {
            return;
        };
        let document = &self.tabs[self.active_tab].scene.document;
        let mut names: Vec<(String, codec::Handle)> = match NAMED_TYPES[state.named_type] {
            "Block" => document
                .block_records
                .iter()
                .filter(|b| !b.name.starts_with('*'))
                .map(|b| (b.name.clone(), b.handle))
                .collect(),
            "Dimstyle" => document.dim_styles.iter().map(|s| (s.name.clone(), s.handle)).collect(),
            "Layer" => document.layers.iter().map(|l| (l.name.clone(), l.handle)).collect(),
            "Linetype" => document.line_types.iter().map(|l| (l.name.clone(), l.handle)).collect(),
            "Textstyle" => document
                .text_styles
                .iter()
                .filter(|s| !s.name.is_empty())
                .map(|s| (s.name.clone(), s.handle))
                .collect(),
            "View" => document.views.iter().map(|v| (v.name.clone(), v.handle)).collect(),
            _ => Vec::new(),
        };
        names.sort_by_key(|(n, _)| n.to_lowercase());
        state.named_names = names;
        state.named = None;
    }

    pub(in crate::app) fn on_field_dialog(&mut self, m: FieldDialogMsg) -> Task<Message> {
        let i = self.active_tab;
        let Some(state) = self.field_dialog.as_mut() else {
            return Task::none();
        };
        match m {
            FieldDialogMsg::Category(c) => {
                state.category = c;
                let names = fields_of(c);
                if !names.contains(&state.name) {
                    state.name = names.first().copied().unwrap_or("Date");
                }
            }
            FieldDialogMsg::Name(n) => {
                state.name = n;
                state.text_case = 0;
            }
            FieldDialogMsg::DateFormat(f) => state.date_format = f,
            FieldDialogMsg::DateExample(k) => {
                if let Some(f) = DATE_FORMATS.get(k) {
                    state.date_format = f.to_string();
                }
            }
            FieldDialogMsg::TextCase(k) => state.text_case = k,
            FieldDialogMsg::FileParts(b) => state.file_parts = b,
            FieldDialogMsg::FileExtension(v) => state.file_extension = v,
            FieldDialogMsg::SizeUnit(k) => state.size_unit = k,
            FieldDialogMsg::SysVar(v) => state.sysvar = v,
            FieldDialogMsg::Diesel(v) => state.diesel = v,
            FieldDialogMsg::NamedType(k) => {
                state.named_type = k;
                self.fill_named_objects();
            }
            FieldDialogMsg::Named(k) => state.named = Some(k),
            FieldDialogMsg::ObjectProp(p) => state.object_prop = Some(p),
            FieldDialogMsg::Formula(v) => state.formula = v,
            FieldDialogMsg::FormulaFormat(k) => state.formula_format = k,
            FieldDialogMsg::FormulaPrecision(k) => state.formula_precision = k,
            FieldDialogMsg::Evaluate => {}
            FieldDialogMsg::HyperlinkText(v) => state.hyperlink_text = v,
            FieldDialogMsg::HyperlinkUrl(v) => state.hyperlink_url = v,
            FieldDialogMsg::BrowseHyperlink => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title(crate::t!("Select a file to link").as_ref())
                            .pick_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    |path| match path {
                        Some(path) => Message::FieldDialog(FieldDialogMsg::HyperlinkUrl(path.to_string_lossy().into_owned())),
                        None => Message::Noop,
                    },
                );
            }
            FieldDialogMsg::PlotScale(k) => state.plot_scale = k,
            FieldDialogMsg::CountExpression(v) => state.count_expression = v,
            FieldDialogMsg::ShowCountInstances => {
                // The dialog closes and count mode shows what the expression counts.
                let json = state.count_expression.clone();
                self.field_dialog = None;
                self.close_active_modal();
                self.show_count_instances(i, &json);
                return Task::none();
            }
            FieldDialogMsg::SelectObject => {
                // The dialog waits while one object is picked.
                self.active_modal = None;
                let command = FieldObjectPickCommand;
                self.command_line.push_info(&command.prompt());
                self.tabs[i].active_cmd = Some(Box::new(command));
                return Task::none();
            }
            FieldDialogMsg::Help => self.command_line.push_info(
                crate::t!("Inserts a field: text that updates from the drawing, the date or another source.").as_ref(),
            ),
            FieldDialogMsg::Ok => return self.field_dialog_ok(i),
        }
        self.refresh_field_preview();
        Task::none()
    }

    fn field_dialog_ok(&mut self, i: usize) -> Task<Message> {
        let Some(state) = self.field_dialog.take() else {
            return Task::none();
        };
        let (code, objects) = state.code();
        let value = state.preview.clone();
        let field = Some((code, objects));
        match state.target {
            FieldTarget::NewText => {
                self.close_active_modal();
                let defaults =
                    crate::scene::creation_style::current_text_defaults(&self.tabs[i].scene.document);
                let command = FieldPlaceCommand::new(
                    value,
                    field.expect("set above"),
                    defaults.style_name,
                    defaults.height,
                );
                self.reset_command_start_state(i);
                self.command_line.push_info(&command.prompt());
                self.tabs[i].active_cmd = Some(Box::new(command));
                self.push_ucs_to_cmd(i);
                return self.focus_cmd_input();
            }
            FieldTarget::AttdefDefault => {
                if let Some(dialog) = self.attdef_dialog.as_mut() {
                    dialog.default = value;
                    dialog.field = field;
                }
                self.active_modal = Some(crate::app::ModalKind::AttDef);
            }
            FieldTarget::AttdefEdit => {
                if let Some(edit) = self.attdef_edit.as_mut() {
                    edit.default = value;
                    edit.field = field;
                    edit.field_changed = true;
                }
                self.active_modal = Some(crate::app::ModalKind::AttDefEdit);
            }
        }
        Task::none()
    }

    /// The dialogs a Field dialog was opened from come back when it closes.
    pub(in crate::app) fn close_field_dialog(&mut self) {
        let target = self.field_dialog.take().map(|s| s.target);
        self.close_active_modal();
        self.active_modal = match target {
            Some(FieldTarget::AttdefDefault) if self.attdef_dialog.is_some() => Some(crate::app::ModalKind::AttDef),
            Some(FieldTarget::AttdefEdit) if self.attdef_edit.is_some() => Some(crate::app::ModalKind::AttDefEdit),
            _ => None,
        };
    }
}
