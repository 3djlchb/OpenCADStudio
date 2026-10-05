//! Sheet sets: SHEETSET / SHEETSETHIDE / NEWSHEETSET / OPENSHEETSET, the
//! SSM* / SS* system variables, the Sheet Set Manager palette and its
//! dialogs, the drawing's sheet link written on save and located on open.

use crate::app::{Message, OpenCADStudio};
use crate::ui::window::sheet_set::{
    FieldId, Form, FormKind, FoundLayout, MenuAction, PropRow, Properties, RowKey, SheetSetMsg, SsDialog,
    Wizard, WizardMsg,
};
use codec::sheet_set::{self as ss, ComponentKind, LayoutReference, SheetSetData, SheetSetDatabase};
use iced::Task;
use std::path::{Path, PathBuf};

/// The variables this family answers for.
pub(super) const SHEET_SET_SYSVARS: &[&str] =
    &["SSMAUTOOPEN", "SSLOCATE", "SSMPOLLTIME", "SSMSHEETSTATUS", "SSMSTATE", "SSFOUND"];

/// Paper-space layouts of a drawing in tab order: name and handle (hex).
fn paper_layouts(doc: &codec::CadDocument) -> Vec<(String, String)> {
    let mut layouts: Vec<(i16, String, String)> = doc
        .objects
        .values()
        .filter_map(|o| match o {
            codec::objects::ObjectType::Layout(l) if !l.name.eq_ignore_ascii_case("Model") => {
                Some((l.tab_order, l.name.clone(), format!("{:X}", l.handle.value())))
            }
            _ => None,
        })
        .collect();
    layouts.sort();
    layouts.into_iter().map(|(_, n, h)| (n, h)).collect()
}

/// UTC now as the reference stamps `UpdateTime`: `yyyy/MM/dd HH:mm:ss.fff`.
fn utc_stamp() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}/{m:02}/{d:02} {:02}:{:02}:{:02}.{:03}",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

fn folder_of(path: &str) -> String {
    Path::new(path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()
}

/// Template of a component for new sheets: its own `DefDwtLayout`, else
/// the nearest parent's.
fn template_of(db: &SheetSetDatabase, component: &str) -> Option<LayoutReference> {
    let mut id = component.to_string();
    for _ in 0..32 {
        if let Some(t) = db.layout_reference(&id, "DefDwtLayout") {
            return Some(t);
        }
        id = db.parent_of(&id)?.id().to_string();
    }
    None
}

/// New sheet folder of a component: its `NewSheetLocation`, else the
/// nearest parent's, else the `.dst` folder.
fn sheet_folder_of(db: &SheetSetDatabase, component: &str) -> String {
    let mut id = component.to_string();
    for _ in 0..32 {
        if let Some(f) = db.file_reference(&id, "NewSheetLocation").filter(|f| !f.is_empty()) {
            return f;
        }
        match db.parent_of(&id) {
            Some(p) => id = p.id().to_string(),
            None => break,
        }
    }
    db.path.as_deref().map(folder_of).unwrap_or_default()
}

impl OpenCADStudio {
    pub(super) fn dispatch_sheet_set(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        let upper = cmd.trim().to_ascii_uppercase();
        let (verb, arg) = match upper.split_once(char::is_whitespace) {
            Some((v, _)) => (v.to_string(), cmd.trim()[v.len()..].trim().to_string()),
            None => (upper.clone(), String::new()),
        };
        match verb.as_str() {
            "SHEETSET" | "SSM" => {
                self.show_sheet_set_manager(true);
                Some(Task::none())
            }
            "SHEETSETHIDE" => {
                self.show_sheet_set_manager(false);
                Some(Task::none())
            }
            // The ribbon toggle (its macro picks SHEETSET or SHEETSETHIDE by SSMSTATE).
            "_SSMTOGGLE" => {
                let on = !self.sheet_set.show;
                self.show_sheet_set_manager(on);
                Some(Task::none())
            }
            "NEWSHEETSET" => {
                self.open_new_sheet_set_wizard();
                Some(Task::none())
            }
            "OPENSHEETSET" => {
                if !arg.is_empty() {
                    return Some(self.update(Message::SheetSet(SheetSetMsg::OpenPicked(Some(PathBuf::from(
                        arg.trim_matches('"'),
                    ))))));
                }
                Some(Task::perform(
                    async {
                        crate::sys::file_dialog()
                            .set_title(crate::t!("Open Sheet Set").as_ref())
                            .add_filter(crate::t!("Sheet Set (*.dst)").as_ref(), &["dst", "DST"])
                            .pick_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    |p| Message::SheetSet(SheetSetMsg::OpenPicked(p)),
                ))
            }
            _ => self.dispatch_sheet_set_vars(cmd, i),
        }
    }

    fn dispatch_sheet_set_vars(&mut self, cmd: &str, _i: usize) -> Option<Task<Message>> {
        let rest = cmd.strip_prefix("SETVAR ").map(str::trim).unwrap_or(cmd);
        let mut parts = rest.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_ascii_uppercase();
        if !SHEET_SET_SYSVARS.contains(&name.as_str()) {
            return None;
        }
        let value = parts.next().map(str::trim).filter(|v| !v.is_empty());
        let s = &mut self.sheet_set.settings;
        match name.as_str() {
            "SSMSTATE" => {
                let v = u8::from(self.sheet_set.show);
                self.command_line.push_output(&format!("SSMSTATE = {v} (read only)"));
            }
            "SSFOUND" => {
                let v = self.sheet_set.found.clone();
                self.command_line.push_output(&format!("SSFOUND = \"{v}\" (read only)"));
            }
            _ => {
                let (current, min, max) = match name.as_str() {
                    "SSMAUTOOPEN" => (i32::from(s.auto_open), 0, 1),
                    "SSLOCATE" => (i32::from(s.locate), 0, 1),
                    "SSMPOLLTIME" => (i32::from(s.poll_time), 20, 600),
                    _ => (i32::from(s.sheet_status), 0, 2),
                };
                let ask = |app: &mut Self, name: String| {
                    app.command_line.push_output(&format!("Enter new value for {name} <{current}>:"));
                    app.pending_setvar = Some(name);
                };
                match value {
                    None => ask(self, name),
                    Some(v) => match v.parse::<i32>() {
                        Err(_) => {
                            self.command_line.push_error("Requires an integer value.");
                            ask(self, name);
                        }
                        Ok(n) if !(min..=max).contains(&n) => {
                            self.command_line.push_error(&if max == 1 {
                                "Requires 0 or 1 only.".to_string()
                            } else {
                                format!("Requires an integer between {min} and {max}.")
                            });
                            ask(self, name);
                        }
                        Ok(n) => {
                            let s = &mut self.sheet_set.settings;
                            match name.as_str() {
                                "SSMAUTOOPEN" => s.auto_open = n as u8,
                                "SSLOCATE" => s.locate = n as u8,
                                "SSMPOLLTIME" => s.poll_time = n as u16,
                                _ => s.sheet_status = n as u8,
                            }
                            self.persist_settings_if_changed();
                        }
                    },
                }
            }
        }
        Some(Task::none())
    }

    /// Show or hide the palette (docked on the right like the other
    /// managers).
    pub(in crate::app) fn show_sheet_set_manager(&mut self, on: bool) {
        let id = crate::ui::dock::PanelId::SheetSetManager;
        self.sheet_set.show = on;
        self.ribbon.set_sheet_set(on);
        if on {
            if self.dock.location(id).is_none() {
                self.dock.dock(id, crate::app::config::DockSide::Right, usize::MAX);
            }
            self.dock_expanded = Some(id);
        } else if self.dock_expanded == Some(id) {
            self.dock_expanded = None;
        }
    }

    /// Publish the open sets to the field engine and redraw every field.
    fn sheet_sets_changed(&mut self) {
        crate::entities::field::set_sheet_sets(self.sheet_set.sets.clone());
        for tab in &mut self.tabs {
            let changes: Vec<_> = tab
                .scene
                .document
                .entities()
                .filter(|e| crate::entities::field::hosts_field(&tab.scene.document, e))
                .map(|e| (e.common().handle, crate::scene::ChangeKind::Modified))
                .collect();
            if !changes.is_empty() {
                tab.scene.bump_entities(&changes);
            }
        }
    }

    /// Open (or bring forward) the sheet set in `path`.
    pub(in crate::app) fn open_sheet_set(&mut self, path: &Path, show: bool) -> Result<(), String> {
        let key = ss::path_key(&path.to_string_lossy());
        if let Some(k) = self
            .sheet_set
            .sets
            .iter()
            .position(|db| db.path.as_deref().is_some_and(|p| ss::path_key(p) == key))
        {
            self.sheet_set.current = Some(k);
        } else {
            let db = SheetSetDatabase::read(&path.to_string_lossy())?;
            self.sheet_set.sets.push(db);
            self.sheet_set.current = Some(self.sheet_set.sets.len() - 1);
        }
        if show {
            self.show_sheet_set_manager(true);
        }
        self.sheet_sets_changed();
        Ok(())
    }

    /// Write the current set back to its `.dst`.
    fn save_current_sheet_set(&mut self) {
        let Some(db) = self.sheet_set.db_mut() else {
            return;
        };
        let Some(path) = db.path.clone() else {
            return;
        };
        if let Err(e) = db.write(&path) {
            self.command_line.push_error(&format!("{path}: {e}"));
        }
        self.sheet_sets_changed();
    }

    fn open_new_sheet_set_wizard(&mut self) {
        let mut n = 1;
        let folder = dirs_next_documents();
        let name = loop {
            let name = format!("New Sheet Set ({n})");
            if !Path::new(&folder).join(format!("{name}.dst")).exists() {
                break name;
            }
            n += 1;
        };
        let draft = SheetSetDatabase::new(&name, "");
        self.sheet_set.dialog = Some(SsDialog::Wizard(Wizard {
            step: 0,
            existing: false,
            name,
            description: String::new(),
            folder,
            hierarchy: false,
            folders: Vec::new(),
            layouts: Vec::new(),
            draft,
            error: None,
        }));
        self.active_modal = Some(crate::app::ModalKind::SheetSet);
    }

    /// Resolve a palette / automation target: a component id, a sheet
    /// number or title, a subset name, or `set`.
    pub(in crate::app) fn sheet_set_target(&self, spec: &str) -> Option<String> {
        let db = self.sheet_set.db()?;
        if spec.eq_ignore_ascii_case("set") {
            return Some(db.sheet_set().id().to_string());
        }
        if db.find(spec).is_some() {
            return Some(spec.to_string());
        }
        fn go(el: &ss::Element, spec: &str) -> Option<String> {
            for c in &el.children {
                match ComponentKind::of(c) {
                    Some(ComponentKind::Sheet)
                        if c.prop("Number") == Some(spec)
                            || c.prop("Title") == Some(spec)
                            || ss::number_and_title(c) == spec =>
                    {
                        return Some(c.id().to_string())
                    }
                    Some(ComponentKind::Subset) => {
                        if c.prop("Name") == Some(spec) {
                            return Some(c.id().to_string());
                        }
                        if let Some(id) = go(c, spec) {
                            return Some(id);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        go(db.sheet_set(), spec)
    }

    fn component_kind(&self, id: &str) -> Option<ComponentKind> {
        ComponentKind::of(self.sheet_set.db()?.find(id)?)
    }

    pub(in crate::app) fn on_sheet_set(&mut self, m: SheetSetMsg) -> Task<Message> {
        match m {
            SheetSetMsg::PickSet(Some(k)) => {
                if k < self.sheet_set.sets.len() {
                    self.sheet_set.current = Some(k);
                }
            }
            SheetSetMsg::PickSet(None) => return self.dispatch_command("OPENSHEETSET"),
            SheetSetMsg::Tab(t) => self.sheet_set.tab = t,
            SheetSetMsg::Expand(id) => {
                if !self.sheet_set.collapsed.remove(&id) {
                    self.sheet_set.collapsed.insert(id);
                }
            }
            SheetSetMsg::Select(id) => self.sheet_set.selected = Some(id),
            SheetSetMsg::Activate(id) => {
                self.sheet_set.selected = Some(id.clone());
                if self.component_kind(&id) == Some(ComponentKind::Sheet) {
                    return self.open_sheet(&id);
                }
                if !self.sheet_set.collapsed.remove(&id) {
                    self.sheet_set.collapsed.insert(id);
                }
            }
            SheetSetMsg::Refresh => {
                if let Some(path) = self.sheet_set.db().and_then(|db| db.path.clone()) {
                    match SheetSetDatabase::read(&path) {
                        Ok(db) => {
                            if let Some(slot) = self.sheet_set.db_mut() {
                                *slot = db;
                            }
                            self.sheet_sets_changed();
                        }
                        Err(e) => self.command_line.push_error(&format!("{path}: {e}")),
                    }
                }
            }
            SheetSetMsg::Menu(id, action) => return self.sheet_set_menu(id, action),
            SheetSetMsg::OpenPicked(Some(path)) => {
                if let Err(e) = self.open_sheet_set(&path, true) {
                    self.command_line.push_error(&format!("{}: {e}", path.display()));
                }
            }
            SheetSetMsg::OpenPicked(None) | SheetSetMsg::ImportPicked(None) | SheetSetMsg::TemplatePicked(None) => {}
            SheetSetMsg::ImportPicked(Some(path)) => self.import_layout_form(path),
            SheetSetMsg::FolderPicked(field, Some(path)) => self.dialog_folder(field, path),
            SheetSetMsg::FolderPicked(_, None) => {}
            SheetSetMsg::TemplatePicked(Some(path)) => self.properties_template(path),
            SheetSetMsg::Browse(field) => {
                return Task::perform(
                    async {
                        crate::sys::file_dialog()
                            .set_title(crate::t!("Browse for Folder").as_ref())
                            .pick_folder()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    move |p| Message::SheetSet(SheetSetMsg::FolderPicked(field, p)),
                )
            }
            SheetSetMsg::BrowseTemplate => {
                return Task::perform(
                    async {
                        crate::sys::file_dialog()
                            .set_title(crate::t!("Select Layout as Sheet Template").as_ref())
                            .add_filter(crate::t!("Drawing Template (*.dwt)").as_ref(), &["dwt", "DWT"])
                            .add_filter(crate::t!("Drawing (*.dwg)").as_ref(), &["dwg", "DWG"])
                            .pick_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    |p| Message::SheetSet(SheetSetMsg::TemplatePicked(p)),
                )
            }
            SheetSetMsg::Input(field, v) => self.dialog_input(field, v),
            SheetSetMsg::Toggle(field, v) => {
                if let (FieldId::Hierarchy, Some(SsDialog::Wizard(w))) = (field, self.sheet_set.dialog.as_mut()) {
                    w.hierarchy = v;
                }
            }
            SheetSetMsg::Choose(field, k) => match (field, self.sheet_set.dialog.as_mut()) {
                (FieldId::Layout, Some(SsDialog::Form(f))) => f.layout = k,
                (FieldId::CustomOwner, Some(SsDialog::Properties(p))) => {
                    if let Some(a) = p.adding.as_mut() {
                        a.2 = k == 1;
                    }
                }
                (FieldId::Row(r), Some(SsDialog::Properties(p))) => {
                    if let Some(row) = p.rows.get_mut(r) {
                        row.value = if k == 0 { "Yes" } else { "No" }.into();
                    }
                }
                _ => {}
            },
            SheetSetMsg::Wizard(w) => self.on_wizard(w),
            SheetSetMsg::WizardProperties => {
                if let Some(SsDialog::Wizard(mut w)) = self.sheet_set.dialog.take() {
                    sync_wizard_draft(&mut w);
                    let set = w.draft.sheet_set().id().to_string();
                    let mut p = build_properties(&w.draft, &set, None);
                    // The set is not written yet: show where it will be.
                    for row in &mut p.rows {
                        match row.key {
                            RowKey::ReadOnly if row.label == "Sheet set data file" => {
                                row.value = crate::ui::window::sheet_set::wizard_dst_path(&w)
                            }
                            RowKey::Folder(_) if row.value.is_empty() => row.value = w.folder.clone(),
                            _ => {}
                        }
                    }
                    p.wizard = Some(Box::new(w));
                    self.sheet_set.dialog = Some(SsDialog::Properties(p));
                }
            }
            SheetSetMsg::AddCustom => {
                if let Some(SsDialog::Properties(p)) = self.sheet_set.dialog.as_mut() {
                    let mut n = 1;
                    while p.custom.iter().any(|(name, _, _)| *name == format!("New Name ({n})")) {
                        n += 1;
                    }
                    p.adding = Some((format!("New Name ({n})"), "Value".into(), false));
                }
            }
            SheetSetMsg::AddCustomCancel => {
                if let Some(SsDialog::Properties(p)) = self.sheet_set.dialog.as_mut() {
                    p.adding = None;
                }
            }
            SheetSetMsg::AddCustomOk => {
                if let Some(SsDialog::Properties(p)) = self.sheet_set.dialog.as_mut() {
                    if let Some((name, value, sheet)) = p.adding.take() {
                        let name = name.trim().to_string();
                        if name.is_empty() {
                            p.error = Some(crate::t!("The name cannot be empty.").into_owned());
                        } else if p.custom.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(&name)) {
                            p.error = Some(crate::t!("A custom property with this name already exists.").into_owned());
                        } else {
                            p.error = None;
                            let flags = if sheet { ss::CUSTOM_SHEET_PROP } else { ss::CUSTOM_SHEET_SET_PROP };
                            p.custom.push((name, value, flags));
                        }
                    }
                }
            }
            SheetSetMsg::CustomValue(k, v) => {
                if let Some(SsDialog::Properties(p)) = self.sheet_set.dialog.as_mut() {
                    if let Some(c) = p.custom.get_mut(k) {
                        c.1 = v;
                    }
                }
            }
            SheetSetMsg::Ok => return self.sheet_set_dialog_ok(),
            SheetSetMsg::Cancel => self.close_sheet_set_dialog(),
            SheetSetMsg::Help => self.command_line.push_info(
                crate::t!("Sheet sets organize layouts from several drawings into one named set of sheets.").as_ref(),
            ),
        }
        Task::none()
    }

    /// Cancel / × of the sheet set dialogs: the Properties dialog opened from
    /// the wizard goes back to it.
    pub(in crate::app) fn close_sheet_set_dialog(&mut self) {
        match self.sheet_set.dialog.take() {
            Some(SsDialog::Properties(p)) if p.wizard.is_some() => {
                self.sheet_set.dialog = p.wizard.map(|w| SsDialog::Wizard(*w));
            }
            _ => self.close_active_modal(),
        }
    }

    fn sheet_set_menu(&mut self, id: String, action: MenuAction) -> Task<Message> {
        let Some(set) = self.sheet_set.current else {
            return Task::none();
        };
        self.sheet_set.selected = Some(id.clone());
        let Some(db) = self.sheet_set.db() else {
            return Task::none();
        };
        match action {
            MenuAction::Open => return self.open_sheet(&id),
            MenuAction::NewSheet => {
                // Next number: the last sheet's number + 1 when it ends in digits.
                let template = template_of(db, &id)
                    .map(|t| format!("{} ({})", t.name, t.file_name))
                    .unwrap_or_else(|| crate::t!("(default new drawing)").into_owned());
                self.sheet_set.dialog = Some(SsDialog::Form(Form {
                    kind: FormKind::NewSheet,
                    set,
                    component: id.clone(),
                    number: String::new(),
                    title: String::new(),
                    file_name: String::new(),
                    folder: sheet_folder_of(db, &id),
                    drawing: template,
                    layouts: Vec::new(),
                    layout: 0,
                    error: None,
                }));
                self.active_modal = Some(crate::app::ModalKind::SheetSet);
            }
            MenuAction::NewSubset => {
                let mut n = 1;
                let parent = db.find(&id);
                while parent.is_some_and(|p| {
                    p.children.iter().any(|c| c.prop("Name") == Some(format!("New Subset ({n})").as_str()))
                }) {
                    n += 1;
                }
                self.sheet_set.dialog = Some(SsDialog::Form(Form {
                    kind: FormKind::NewSubset,
                    set,
                    component: id.clone(),
                    number: String::new(),
                    title: format!("New Subset ({n})"),
                    file_name: String::new(),
                    folder: sheet_folder_of(db, &id),
                    drawing: String::new(),
                    layouts: Vec::new(),
                    layout: 0,
                    error: None,
                }));
                self.active_modal = Some(crate::app::ModalKind::SheetSet);
            }
            MenuAction::ImportLayout => {
                self.sheet_set_import_parent = Some(id);
                return Task::perform(
                    async {
                        crate::sys::file_dialog()
                            .set_title(crate::t!("Select Drawing").as_ref())
                            .add_filter(crate::t!("Drawing (*.dwg)").as_ref(), &["dwg", "DWG"])
                            .pick_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    |p| Message::SheetSet(SheetSetMsg::ImportPicked(p)),
                );
            }
            MenuAction::Rename => {
                let el = db.find(&id);
                self.sheet_set.dialog = Some(SsDialog::Form(Form {
                    kind: FormKind::Rename,
                    set,
                    component: id.clone(),
                    number: el.and_then(|e| e.prop("Number")).unwrap_or("").to_string(),
                    title: el.and_then(|e| e.prop("Title")).unwrap_or("").to_string(),
                    file_name: String::new(),
                    folder: String::new(),
                    drawing: String::new(),
                    layouts: Vec::new(),
                    layout: 0,
                    error: None,
                }));
                self.active_modal = Some(crate::app::ModalKind::SheetSet);
            }
            MenuAction::Remove => {
                if let Some(db) = self.sheet_set.db_mut() {
                    db.remove(&id);
                }
                self.sheet_set.selected = None;
                self.save_current_sheet_set();
            }
            MenuAction::Properties => {
                let p = build_properties(db, &id, Some(set));
                self.sheet_set.dialog = Some(SsDialog::Properties(p));
                self.active_modal = Some(crate::app::ModalKind::SheetSet);
            }
            MenuAction::Close => {
                self.sheet_set.sets.remove(set);
                self.sheet_set.current = (!self.sheet_set.sets.is_empty()).then_some(0);
                self.sheet_set.selected = None;
                self.sheet_sets_changed();
            }
        }
        Task::none()
    }

    /// Open a sheet's drawing (or switch to it) at the sheet's layout.
    fn open_sheet(&mut self, id: &str) -> Task<Message> {
        let Some(reference) = self.sheet_set.db().and_then(|db| db.layout_reference(id, "Layout")) else {
            return Task::none();
        };
        let path = PathBuf::from(&reference.file_name);
        if let Some(k) = self.tab_showing(&path) {
            return Task::batch([
                self.update(Message::TabSwitch(k)),
                Task::done(Message::LayoutSwitch(reference.name)),
            ]);
        }
        if !path.exists() {
            self.command_line
                .push_error(crate::tf!("Sheet drawing not found: {}", path.display()).as_ref());
            return Task::none();
        }
        self.sheet_set_pending_layout = Some((ss::path_key(&reference.file_name), reference.name));
        self.update(Message::OpenRecent(path))
    }

    /// After a drawing opened: switch to the sheet layout that was asked for,
    /// and locate the sheet set the drawing names (SSLOCATE / SSMAUTOOPEN).
    pub(in crate::app) fn sheet_set_after_open(&mut self, i: usize) -> Task<Message> {
        let mut task = Task::none();
        let path = self.tabs[i].current_path.as_ref().map(|p| p.to_string_lossy().to_string());
        if let (Some(path), Some((want, layout))) = (path.as_ref(), self.sheet_set_pending_layout.as_ref()) {
            if ss::path_key(path) == *want {
                task = Task::done(Message::LayoutSwitch(layout.clone()));
                self.sheet_set_pending_layout = None;
            }
        }
        if self.sheet_set.settings.locate == 1 {
            if let Some(data) = self.tabs[i].scene.document.sheet_set_data() {
                let named = PathBuf::from(&data.sheet_set_file_name);
                // A moved set is looked for next to the drawing.
                let found = if named.exists() {
                    Some(named)
                } else {
                    path.as_deref().and_then(|p| {
                        let beside = Path::new(p).parent()?.join(named.file_name()?);
                        beside.exists().then_some(beside)
                    })
                };
                self.sheet_set.found = found.as_ref().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                if let Some(found) = found {
                    let show = self.sheet_set.settings.auto_open == 1;
                    if let Err(e) = self.open_sheet_set(&found, show) {
                        self.command_line.push_error(&format!("{}: {e}", found.display()));
                    }
                }
            }
        }
        task
    }

    /// Before a native save: a drawing that is a sheet of an open set records
    /// its link (`AcSheetSetData`), as the reference does when it saves a
    /// sheet while its set is open.
    pub(in crate::app) fn stamp_sheet_set_data(&mut self, i: usize) {
        let Some(path) = self.tabs[i].current_path.as_ref().map(|p| p.to_string_lossy().to_string()) else {
            return;
        };
        let layout = self.tabs[i].scene.current_layout.clone();
        let layouts = paper_layouts(&self.tabs[i].scene.document);
        for db in &self.sheet_set.sets {
            let wanted = (!layout.eq_ignore_ascii_case("Model")).then_some(layout.as_str());
            let Some(sheet) = db.sheet_for(&path, wanted) else {
                continue;
            };
            let Some(reference) = db.layout_reference(sheet.id(), "Layout") else {
                continue;
            };
            let handle = layouts
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&reference.name))
                .map(|(_, h)| h.to_lowercase())
                .unwrap_or_else(|| reference.handle.to_lowercase());
            let doc = &mut self.tabs[i].scene.document;
            let previous = doc.sheet_set_data().unwrap_or_default();
            doc.set_sheet_set_data(&SheetSetData {
                layout_handle: handle,
                layout_name: reference.name,
                sheet_dwg_name: ss::native_path(&path),
                sheet_set_file_name: ss::native_path(db.path.as_deref().unwrap_or_default()),
                sheet_set_version: db.file_revision(),
                update_count: previous.update_count + 1,
                update_time: utc_stamp(),
            });
            return;
        }
    }

    fn dialog_input(&mut self, field: FieldId, v: String) {
        match self.sheet_set.dialog.as_mut() {
            Some(SsDialog::Wizard(w)) => match field {
                FieldId::Name => w.name = v,
                FieldId::Description => w.description = v,
                FieldId::Folder => w.folder = v,
                _ => {}
            },
            Some(SsDialog::Properties(p)) => match field {
                FieldId::Row(r) => {
                    if let Some(row) = p.rows.get_mut(r) {
                        row.value = v;
                    }
                }
                FieldId::CustomName => {
                    if let Some(a) = p.adding.as_mut() {
                        a.0 = v;
                    }
                }
                FieldId::CustomDefault => {
                    if let Some(a) = p.adding.as_mut() {
                        a.1 = v;
                    }
                }
                _ => {}
            },
            Some(SsDialog::Form(f)) => {
                let file_follows = f.file_name == format!("{} {}", f.number, f.title).trim();
                match field {
                    FieldId::Number => f.number = v,
                    FieldId::Title => f.title = v,
                    FieldId::FileName => f.file_name = v,
                    FieldId::Folder => f.folder = v,
                    _ => {}
                }
                // The file name follows "Number Title" until edited.
                if f.kind == FormKind::NewSheet && file_follows && field != FieldId::FileName {
                    f.file_name = format!("{} {}", f.number, f.title).trim().to_string();
                }
            }
            None => {}
        }
    }

    fn dialog_folder(&mut self, field: FieldId, path: PathBuf) {
        let folder = path.to_string_lossy().to_string();
        if field == FieldId::AddFolder {
            self.wizard_add_folder(path);
            return;
        }
        self.dialog_input(field, folder);
    }

    fn wizard_add_folder(&mut self, path: PathBuf) {
        let Some(SsDialog::Wizard(w)) = self.sheet_set.dialog.as_mut() else {
            return;
        };
        let folder = path.to_string_lossy().to_string();
        if w.folders.contains(&folder) {
            return;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dwg")))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for file in files {
            let Ok(doc) = crate::io::load_file(&file) else {
                continue;
            };
            for (layout, handle) in paper_layouts(&doc) {
                w.layouts.push(FoundLayout {
                    folder: folder.clone(),
                    file: file.to_string_lossy().to_string(),
                    layout,
                    handle,
                    on: true,
                });
            }
        }
        w.folders.push(folder);
    }

    fn on_wizard(&mut self, m: WizardMsg) {
        let Some(SsDialog::Wizard(w)) = self.sheet_set.dialog.as_mut() else {
            return;
        };
        match m {
            WizardMsg::Step(s) => {
                w.error = None;
                if s > 1 && w.step <= 1 {
                    if w.name.trim().is_empty() {
                        w.error = Some(crate::t!("The sheet set name cannot be empty.").into_owned());
                        return;
                    }
                    if !Path::new(&w.folder).is_dir() {
                        w.error = Some(crate::t!("The folder for the sheet set data file does not exist.").into_owned());
                        return;
                    }
                }
                w.step = if s == 2 && !w.existing { 3 } else { s };
                sync_wizard_draft(w);
            }
            WizardMsg::Existing(on) => w.existing = on,
            WizardMsg::LayoutOn(k, on) => {
                if let Some(l) = w.layouts.get_mut(k) {
                    l.on = on;
                }
            }
            WizardMsg::RemoveFolder(k) => {
                if k < w.folders.len() {
                    let folder = w.folders.remove(k);
                    w.layouts.retain(|l| l.folder != folder);
                }
            }
        }
    }

    fn import_layout_form(&mut self, path: PathBuf) {
        let (Some(set), Some(parent)) = (self.sheet_set.current, self.sheet_set_import_parent.take()) else {
            return;
        };
        let layouts = match crate::io::load_file(&path) {
            Ok(doc) => paper_layouts(&doc),
            Err(e) => {
                self.command_line.push_error(&format!("{}: {e}", path.display()));
                return;
            }
        };
        self.sheet_set.dialog = Some(SsDialog::Form(Form {
            kind: FormKind::ImportLayout,
            set,
            component: parent,
            number: String::new(),
            title: String::new(),
            file_name: String::new(),
            folder: String::new(),
            drawing: path.to_string_lossy().to_string(),
            layouts,
            layout: 0,
            error: None,
        }));
        self.active_modal = Some(crate::app::ModalKind::SheetSet);
    }

    fn properties_template(&mut self, path: PathBuf) {
        let layouts = crate::io::load_file(&path).map(|d| paper_layouts(&d)).unwrap_or_default();
        let Some(SsDialog::Properties(p)) = self.sheet_set.dialog.as_mut() else {
            return;
        };
        match layouts.first() {
            Some((name, _)) => {
                let file = path.to_string_lossy().to_string();
                if let Some(row) = p.rows.iter_mut().find(|r| r.key == RowKey::Template) {
                    row.value = format!("{name} ({file})");
                }
                p.template = Some((file, name.clone()));
                p.error = None;
            }
            None => p.error = Some(crate::t!("The drawing has no layout to use as a sheet template.").into_owned()),
        }
    }

    fn sheet_set_dialog_ok(&mut self) -> Task<Message> {
        match self.sheet_set.dialog.take() {
            Some(SsDialog::Wizard(w)) => self.finish_wizard(w),
            Some(SsDialog::Properties(p)) => {
                self.apply_properties(p);
                Task::none()
            }
            Some(SsDialog::Form(f)) => self.form_ok(f),
            None => Task::none(),
        }
    }

    fn finish_wizard(&mut self, mut w: Wizard) -> Task<Message> {
        sync_wizard_draft(&mut w);
        let path = crate::ui::window::sheet_set::wizard_dst_path(&w);
        if w.name.trim().is_empty() || !Path::new(&w.folder).is_dir() {
            w.error = Some(crate::t!("The folder for the sheet set data file does not exist.").into_owned());
            self.sheet_set.dialog = Some(SsDialog::Wizard(w));
            return Task::none();
        }
        if Path::new(&path).exists() {
            w.error = Some(crate::tf!("{} already exists.", path).into_owned());
            self.sheet_set.dialog = Some(SsDialog::Wizard(w));
            return Task::none();
        }
        let mut db = w.draft.clone();
        db.path = Some(path.clone());
        let set = db.sheet_set().id().to_string();
        // Written again now the `.dst` has a place, so it carries its relative path.
        let location = db
            .file_reference(&set, "NewSheetLocation")
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| w.folder.clone());
        db.set_file_reference(&set, "NewSheetLocation", &location);
        // Existing drawings: a subset per folder, a sheet per chosen layout.
        if w.existing {
            for folder in &w.folders {
                let chosen: Vec<&FoundLayout> = w.layouts.iter().filter(|l| l.on && &l.folder == folder).collect();
                if chosen.is_empty() {
                    continue;
                }
                let name = Path::new(folder).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let Some(sub) = db.add_subset(&set, &name, "") else {
                    continue;
                };
                if w.hierarchy {
                    db.set_file_reference(&sub, "NewSheetLocation", folder);
                }
                for l in chosen {
                    let stem = Path::new(&l.file).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    if let Some(sheet) = db.add_sheet(&sub, "", &format!("{stem} - {}", l.layout), "") {
                        db.set_layout_reference(
                            &sheet,
                            "Layout",
                            &LayoutReference { file_name: l.file.clone(), name: l.layout.clone(), handle: l.handle.clone() },
                        );
                    }
                }
            }
        }
        if let Err(e) = db.write(&path) {
            w.error = Some(e);
            self.sheet_set.dialog = Some(SsDialog::Wizard(w));
            return Task::none();
        }
        self.close_active_modal();
        if let Err(e) = self.open_sheet_set(Path::new(&path), true) {
            self.command_line.push_error(&e);
        }
        Task::none()
    }

    fn apply_properties(&mut self, mut p: Properties) {
        // Back to the wizard: the draft takes the edits.
        if let Some(mut w) = p.wizard.take() {
            write_properties(&mut w.draft, &p);
            let set = w.draft.sheet_set();
            w.name = set.prop("Name").unwrap_or("").to_string();
            w.description = set.prop("Desc").unwrap_or("").to_string();
            self.sheet_set.dialog = Some(SsDialog::Wizard(*w));
            return;
        }
        self.close_active_modal();
        let Some(db) = p.set.and_then(|k| self.sheet_set.sets.get_mut(k)) else {
            return;
        };
        write_properties(db, &p);
        self.sheet_set.current = p.set;
        self.save_current_sheet_set();
    }

    fn form_ok(&mut self, mut f: Form) -> Task<Message> {
        let Some(db) = self.sheet_set.sets.get_mut(f.set) else {
            self.close_active_modal();
            return Task::none();
        };
        match f.kind {
            FormKind::Rename => {
                if let Some(el) = db.find_mut(&f.component) {
                    el.set_prop("Number", f.number.trim());
                    el.set_prop("Title", f.title.trim());
                }
            }
            FormKind::NewSubset => {
                let name = f.title.trim().to_string();
                if name.is_empty() {
                    f.error = Some(crate::t!("The subset name cannot be empty.").into_owned());
                    self.sheet_set.dialog = Some(SsDialog::Form(f));
                    return Task::none();
                }
                if let Some(sub) = db.add_subset(&f.component, &name, "") {
                    if !f.folder.trim().is_empty() {
                        db.set_file_reference(&sub, "NewSheetLocation", f.folder.trim());
                    }
                }
            }
            FormKind::ImportLayout => {
                let Some((layout, handle)) = f.layouts.get(f.layout).cloned() else {
                    f.error = Some(crate::t!("The drawing has no layout to import.").into_owned());
                    self.sheet_set.dialog = Some(SsDialog::Form(f));
                    return Task::none();
                };
                if db.sheet_for(&f.drawing, Some(&layout)).is_some_and(|s| {
                    s.named("Layout").and_then(|r| r.prop("Name")).is_some_and(|n| n.eq_ignore_ascii_case(&layout))
                }) {
                    f.error = Some(crate::t!("The layout is already a sheet of this sheet set.").into_owned());
                    self.sheet_set.dialog = Some(SsDialog::Form(f));
                    return Task::none();
                }
                let stem = Path::new(&f.drawing).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                if let Some(sheet) = db.add_sheet(&f.component, "", &format!("{stem} - {layout}"), "") {
                    db.set_layout_reference(
                        &sheet,
                        "Layout",
                        &LayoutReference { file_name: f.drawing.clone(), name: layout, handle },
                    );
                }
            }
            FormKind::NewSheet => return self.create_sheet(f),
        }
        self.close_active_modal();
        self.sheet_set.current = Some(f.set);
        self.save_current_sheet_set();
        Task::none()
    }

    /// New Sheet: a drawing in the sheet folder holding one layout, named
    /// "Number Title", from the sheet creation template (a new drawing's
    /// Layout1 when the set names none); then the sheet and its layout
    /// reference, and the drawing opens at it.
    fn create_sheet(&mut self, mut f: Form) -> Task<Message> {
        let fail = |app: &mut Self, mut f: Form, e: String| {
            f.error = Some(e);
            app.sheet_set.dialog = Some(SsDialog::Form(f));
            Task::none()
        };
        let title = f.title.trim().to_string();
        if title.is_empty() {
            return fail(self, f, crate::t!("The sheet title cannot be empty.").into_owned());
        }
        let stem = if f.file_name.trim().is_empty() {
            format!("{} {}", f.number.trim(), title).trim().to_string()
        } else {
            f.file_name.trim().trim_end_matches(".dwg").to_string()
        };
        let folder = PathBuf::from(f.folder.trim());
        if std::fs::create_dir_all(&folder).is_err() {
            return fail(self, f, crate::t!("The sheet folder cannot be created.").into_owned());
        }
        let path = folder.join(format!("{stem}.dwg"));
        if path.exists() {
            return fail(self, f, crate::tf!("{} already exists.", path.display()).into_owned());
        }
        let layout_name = format!("{} {}", f.number.trim(), title).trim().to_string();
        let template = self.sheet_set.sets.get(f.set).and_then(|db| template_of(db, &f.component));
        let mut scene = crate::scene::Scene::new();
        let source_layout = match template.as_ref().and_then(|t| crate::io::load_file(Path::new(&t.file_name)).ok().map(|d| (d, t.name.clone()))) {
            Some((doc, name)) => {
                scene.document = doc;
                name
            }
            None => {
                scene.populate_new_drawing_defaults();
                paper_layouts(&scene.document).into_iter().next().map(|(n, _)| n).unwrap_or_else(|| "Layout1".into())
            }
        };
        for (name, _) in paper_layouts(&scene.document) {
            if !name.eq_ignore_ascii_case(&source_layout) {
                scene.delete_layout(&name);
            }
        }
        scene.rename_layout(&source_layout, &layout_name);
        let handle = paper_layouts(&scene.document)
            .into_iter()
            .find(|(n, _)| *n == layout_name)
            .map(|(_, h)| h)
            .unwrap_or_default();
        f.number = f.number.trim().to_string();
        let Some(db) = self.sheet_set.sets.get_mut(f.set) else {
            return Task::none();
        };
        let Some(sheet) = db.add_sheet(&f.component, &f.number, &title, "") else {
            return Task::none();
        };
        let file = ss::native_path(&path.to_string_lossy());
        db.set_layout_reference(
            &sheet,
            "Layout",
            &LayoutReference { file_name: file.clone(), name: layout_name.clone(), handle: handle.clone() },
        );
        let dst = db.path.clone().unwrap_or_default();
        if let Err(e) = db.write(&dst) {
            self.command_line.push_error(&format!("{dst}: {e}"));
        }
        let revision = db.file_revision();
        scene.document.set_sheet_set_data(&SheetSetData {
            layout_handle: handle.to_lowercase(),
            layout_name,
            sheet_dwg_name: file.clone(),
            sheet_set_file_name: ss::native_path(&dst),
            sheet_set_version: revision,
            update_count: 1,
            update_time: utc_stamp(),
        });
        crate::entities::field::stamp_save_dates(&mut scene.document);
        if let Err(e) = crate::io::save(&scene.document, &path) {
            self.command_line.push_error(&format!("{file}: {e}"));
        }
        self.close_active_modal();
        self.sheet_set.current = Some(f.set);
        self.sheet_set.selected = Some(sheet.clone());
        self.sheet_sets_changed();
        self.open_sheet(&sheet)
    }
}

/// The reference's default folder for new sheet set data: My Documents.
fn dirs_next_documents() -> String {
    #[cfg(target_os = "windows")]
    if let Ok(profile) = std::env::var("USERPROFILE") {
        for sub in ["Documents", "OneDrive\\Belgeler", "OneDrive\\Documents"] {
            let p = Path::new(&profile).join(sub);
            if p.is_dir() {
                return p.to_string_lossy().to_string();
            }
        }
        return profile;
    }
    std::env::var("HOME").unwrap_or_default()
}

/// Copy the wizard's name and description into its draft set.
fn sync_wizard_draft(w: &mut Wizard) {
    let set = w.draft.sheet_set_mut();
    set.set_prop("Name", w.name.trim());
    set.set_prop("Desc", &w.description);
}

/// The Properties rows for component `id` of `db` (`set` = its index among
/// the open sets; `None` for the wizard's draft).
pub(in crate::app) fn build_properties(db: &SheetSetDatabase, id: &str, set: Option<usize>) -> Properties {
    let el = db.find(id).cloned().unwrap_or_default();
    let kind = ComponentKind::of(&el).unwrap_or(ComponentKind::SheetSet);
    let prop = |name: &str| el.prop(name).unwrap_or("").to_string();
    let row = |group: &'static str, label: &'static str, key: RowKey, value: String| PropRow { group, label, key, value };
    let template = || {
        db.layout_reference(id, "DefDwtLayout")
            .map(|t| format!("{} ({})", t.name, t.file_name))
            .unwrap_or_default()
    };
    let prompt = || if el.prop("PromptForDwt").is_some_and(|v| v.trim() != "0") { "Yes" } else { "No" }.to_string();
    let named_file = |name: &str| el.named(name).map(|r| db.resolve_file(r)).unwrap_or_default();
    let mut rows = Vec::new();
    let title;
    match kind {
        ComponentKind::SheetSet => {
            title = format!("{} - {}", crate::t!("Sheet Set Properties"), prop("Name"));
            rows.push(row("Sheet Set", "Name", RowKey::Prop("Name"), prop("Name")));
            rows.push(row("Sheet Set", "Sheet set data file", RowKey::ReadOnly, db.path.clone().unwrap_or_default()));
            rows.push(row("Sheet Set", "Description", RowKey::Prop("Desc"), prop("Desc")));
            let resources = el
                .named("Resources")
                .map(|r| r.children.iter().map(|c| db.resolve_file(c)).collect::<Vec<_>>().join("; "))
                .unwrap_or_default();
            rows.push(row("Sheet Set", "Model view", RowKey::ReadOnly, resources));
            let label_block = el
                .named("DefLabelBlk")
                .map(|r| format!("{} ({})", r.prop("Name").unwrap_or(""), db.resolve_file(r)))
                .unwrap_or_default();
            rows.push(row("Sheet Set", "Label block for views", RowKey::ReadOnly, label_block));
            let callouts = el
                .named("CalloutBlocks")
                .map(|r| r.children.iter().filter_map(|c| c.prop("Name")).collect::<Vec<_>>().join("; "))
                .unwrap_or_default();
            rows.push(row("Sheet Set", "Callout blocks", RowKey::ReadOnly, callouts));
            rows.push(row("Sheet Set", "Page setup overrides file", RowKey::ReadOnly, named_file("AltPageSetups")));
            for (label, name) in [
                ("Project number", "ProjectNumber"),
                ("Project name", "ProjectName"),
                ("Project phase", "ProjectPhase"),
                ("Project milestone", "ProjectMilestone"),
            ] {
                rows.push(row("Project Control", label, RowKey::Prop(name), prop(name)));
            }
            rows.push(row("Sheet Creation", "Sheet storage location", RowKey::Folder("NewSheetLocation"), named_file("NewSheetLocation")));
            rows.push(row("Sheet Creation", "Sheet creation template", RowKey::Template, template()));
            rows.push(row("Sheet Creation", "Prompt for template", RowKey::PromptTemplate, prompt()));
        }
        ComponentKind::Subset => {
            title = format!("{} - {}", crate::t!("Subset Properties"), prop("Name"));
            rows.push(row("Subset", "Subset name", RowKey::Prop("Name"), prop("Name")));
            rows.push(row("Subset", "Description", RowKey::Prop("Desc"), prop("Desc")));
            rows.push(row("Subset", "New sheet location", RowKey::Folder("NewSheetLocation"), named_file("NewSheetLocation")));
            rows.push(row("Subset", "Sheet creation template", RowKey::Template, template()));
            rows.push(row("Subset", "Prompt for template", RowKey::PromptTemplate, prompt()));
        }
        ComponentKind::Sheet => {
            title = format!("{} - {}", crate::t!("Sheet Properties"), ss::number_and_title(&el));
            for (label, name) in [
                ("Title", "Title"),
                ("Number", "Number"),
                ("Description", "Desc"),
                ("Revision number", "RevisionNumber"),
                ("Revision date", "RevisionDate"),
                ("Issue purpose", "IssuePurpose"),
                ("Category", "Category"),
            ] {
                rows.push(row("Sheet", label, RowKey::Prop(name), prop(name)));
            }
            let layout = db.layout_reference(id, "Layout").unwrap_or_default();
            rows.push(row("Sheet", "Layout name", RowKey::ReadOnly, layout.name));
            rows.push(row("Sheet", "Drawing file", RowKey::ReadOnly, layout.file_name));
        }
    }
    // A sheet shows the set's sheet properties, with its own value when set.
    let custom = match kind {
        ComponentKind::Sheet => {
            let own = ss::custom_properties(&el);
            ss::custom_properties(db.sheet_set())
                .into_iter()
                .filter(|(_, _, f)| f & ss::CUSTOM_SHEET_PROP != 0)
                .map(|(n, v, f)| {
                    let value = own.iter().find(|(o, _, _)| *o == n).map(|(_, v, _)| v.clone()).unwrap_or(v);
                    (n, value, f)
                })
                .collect()
        }
        ComponentKind::SheetSet => ss::custom_properties(&el),
        ComponentKind::Subset => Vec::new(),
    };
    Properties {
        set,
        component: id.to_string(),
        kind,
        title,
        rows,
        custom,
        adding: None,
        template: None,
        wizard: None,
        error: None,
    }
}

/// Write a Properties dialog back into its component.
fn write_properties(db: &mut SheetSetDatabase, p: &Properties) {
    for r in &p.rows {
        match &r.key {
            RowKey::Prop(name) => {
                if let Some(el) = db.find_mut(&p.component) {
                    el.set_prop(name, &r.value);
                }
            }
            RowKey::Folder(name) => {
                db.set_file_reference(&p.component, name, r.value.trim());
            }
            RowKey::PromptTemplate => {
                if let Some(el) = db.find_mut(&p.component) {
                    if r.value == "Yes" {
                        el.set_prop_vt("PromptForDwt", 3, "1");
                    } else {
                        el.remove_named("PromptForDwt");
                    }
                }
            }
            RowKey::Template | RowKey::ReadOnly => {}
        }
    }
    if let Some((file, name)) = &p.template {
        db.set_layout_reference(
            &p.component,
            "DefDwtLayout",
            &LayoutReference { file_name: file.clone(), name: name.clone(), handle: String::new() },
        );
    }
    match p.kind {
        ComponentKind::SheetSet => {
            if let Some(el) = db.find_mut(&p.component) {
                let old: Vec<String> = ss::custom_properties(el).into_iter().map(|(n, _, _)| n).collect();
                for n in old.iter().filter(|n| !p.custom.iter().any(|(c, _, _)| c == *n)) {
                    ss::remove_custom_property(el, n);
                }
                for (n, v, f) in &p.custom {
                    ss::set_custom_property(el, n, v, *f);
                }
            }
        }
        ComponentKind::Sheet => {
            if let Some(el) = db.find_mut(&p.component) {
                for (n, v, f) in &p.custom {
                    ss::set_custom_property(el, n, v, *f);
                }
            }
        }
        ComponentKind::Subset => {}
    }
}

