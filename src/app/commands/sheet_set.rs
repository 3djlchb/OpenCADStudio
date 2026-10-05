//! Sheet sets: SHEETSET / SHEETSETHIDE / NEWSHEETSET / OPENSHEETSET, the
//! SSM* / SS* system variables, the Sheet Set Manager palette and its
//! dialogs, the drawing's sheet link written on save and located on open.

use crate::app::{Message, OpenCADStudio};
use crate::ui::window::sheet_set::{
    Category, FieldId, Form, FormKind, FoundLayout, ImportRow, MenuAction, PropRow, Properties, RowKey, SheetSetMsg,
    SheetStatus, SsDialog, TemplatePick, Wizard, WizardMsg,
};
use codec::sheet_set::{self as ss, ComponentKind, LayoutReference, SheetSetData, SheetSetDatabase};
use iced::Task;
use std::collections::HashSet;
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
/// The lock files' date: the long date of the user's locale, two spaces and
/// the 24-hour time (`5 Ekim 2026 Pazartesi  09:26:39`).
#[cfg(windows)]
fn lock_stamp() -> String {
    use windows_sys::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, DATE_LONGDATE};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let mut date = [0u16; 128];
    let mut time = [0u16; 64];
    let format = wide("HH:mm:ss");
    // SAFETY: the buffers outlive the calls and their lengths are passed.
    let (d, t) = unsafe {
        (
            GetDateFormatEx(std::ptr::null(), DATE_LONGDATE, std::ptr::null(), std::ptr::null(), date.as_mut_ptr(), date.len() as i32, std::ptr::null()),
            GetTimeFormatEx(std::ptr::null(), 0, std::ptr::null(), format.as_ptr(), time.as_mut_ptr(), time.len() as i32),
        )
    };
    let text = |b: &[u16], n: i32| String::from_utf16_lossy(&b[..(n.max(1) as usize - 1)]);
    format!("{}  {}", text(&date, d), text(&time, t))
}

#[cfg(not(windows))]
fn lock_stamp() -> String {
    String::new()
}

/// `<drawing>.dwl` (user, machine, date lines) and `<drawing>.dwl2` (the same
/// as XML), as the reference writes them for a drawing it has open.
// ponytail: .dwl is written as UTF-8; the reference's encoding of non-ASCII
// day names was not measured.
#[cfg(not(target_arch = "wasm32"))]
fn write_lock(path: &Path) -> std::io::Result<()> {
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default();
    let stamp = lock_stamp();
    std::fs::write(path.with_extension("dwl"), format!("{user}\n{machine} \n{stamp}"))?;
    std::fs::write(
        path.with_extension("dwl2"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\">\n<whprops>\n<username>{user}</username>\n<machinename>{machine} </machinename>\n<fullname></fullname>\n<datetime>{stamp}</datetime>\n</whprops>"
        ),
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub(in crate::app) fn release_lock(path: &Path) {
    let _ = std::fs::remove_file(path.with_extension("dwl"));
    let _ = std::fs::remove_file(path.with_extension("dwl2"));
}

/// Whether new sheets of `component` prompt for their template: its own
/// "Prompt for template", else the nearest parent's.
fn prompts_for_template(db: &SheetSetDatabase, component: &str) -> bool {
    let mut id = component.to_string();
    for _ in 0..32 {
        let Some(el) = db.find(&id) else { return false };
        if let Some(v) = el.prop("PromptForDwt") {
            return !matches!(v.trim(), "" | "0");
        }
        match db.parent_of(&id) {
            Some(p) => id = p.id().to_string(),
            None => return false,
        }
    }
    false
}

/// The layouts of a drawing that can become sheets, each with the sheet set
/// it already belongs to: the drawing's sheet link, or this set.
fn import_rows(path: &Path, db: &SheetSetDatabase) -> Result<Vec<ImportRow>, String> {
    let doc = crate::io::load_file(path).map_err(|e| e.to_string())?;
    let drawing = path.to_string_lossy().to_string();
    let link = doc.sheet_set_data();
    Ok(paper_layouts(&doc)
        .into_iter()
        .map(|(layout, handle)| {
            let mine = db.sheet_for(&drawing, Some(&layout)).is_some_and(|s| {
                s.named("Layout").and_then(|r| r.prop("Name")).is_some_and(|n| n.eq_ignore_ascii_case(&layout))
            });
            // A layout of this set is taken; one the drawing's sheet link
            // names for another set is only warned about (still importable).
            let warn = !mine
                && link.as_ref().is_some_and(|l| {
                    l.layout_name.eq_ignore_ascii_case(&layout)
                        && !l.sheet_set_file_name.is_empty()
                        && db.path.as_deref().is_none_or(|p| ss::path_key(p) != ss::path_key(&l.sheet_set_file_name))
                });
            ImportRow { drawing: drawing.clone(), layout, handle, on: !mine, taken: mine, warn }
        })
        .collect())
}

/// Rename & Renumber: layout and file names follow the options.
fn follow_rename(f: &mut Form) {
    let name = |prefix: bool| if prefix { format!("{} {}", f.number.trim(), f.title.trim()).trim().to_string() } else { f.title.trim().to_string() };
    if f.rename[0] {
        f.layout_name = name(f.rename[1]);
    }
    if f.rename[2] {
        f.file_name = format!("{}.dwg", name(f.rename[3]));
    }
}

fn rename_form(db: &SheetSetDatabase, set: usize, id: &str) -> Form {
    let el = db.find(id);
    let reference = db.layout_reference(id, "Layout").unwrap_or_default();
    let file = Path::new(&reference.file_name);
    Form {
        kind: FormKind::Rename,
        set,
        component: id.to_string(),
        number: el.and_then(|e| e.prop("Number")).unwrap_or("").to_string(),
        title: el.and_then(|e| e.prop("Title")).unwrap_or("").to_string(),
        layout_name: reference.name.clone(),
        file_name: file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        folder: file.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        ..Form::default()
    }
}

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
        self.refresh_locations();
        if self.sheet_set.settings.sheet_status >= 1 {
            self.refresh_sheet_status();
        } else {
            self.sheet_set.status.clear();
        }
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

    /// Lock files for the drawings open in tabs, as other instances expect
    /// them (the sheet status shows such a sheet as open); a drawing that
    /// already has someone else's lock file is left alone.
    #[cfg(not(target_arch = "wasm32"))]
    fn sync_drawing_locks(&mut self) {
        let open: HashSet<PathBuf> = self.tabs.iter().filter_map(|t| t.current_path.clone()).filter(|p| p.exists()).collect();
        let gone: Vec<PathBuf> = self.sheet_set.locks.difference(&open).cloned().collect();
        for p in gone {
            release_lock(&p);
            self.sheet_set.locks.remove(&p);
        }
        for p in open {
            if self.sheet_set.locks.contains(&p) || p.with_extension("dwl").exists() {
                continue;
            }
            if write_lock(&p).is_ok() {
                self.sheet_set.locks.insert(p);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn sync_drawing_locks(&mut self) {}

    /// Reload every open set whose `.dst` was changed by someone else.
    fn poll_sheet_sets(&mut self) {
        let mut changed = false;
        for db in &mut self.sheet_set.sets {
            let Some(path) = db.path.clone() else {
                continue;
            };
            let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
                continue;
            };
            let key = ss::path_key(&path);
            if self.sheet_set.seen.insert(key, modified).is_none_or(|t| t == modified) {
                continue;
            }
            if let Ok(fresh) = SheetSetDatabase::read(&path) {
                if fresh.root != db.root {
                    *db = fresh;
                    changed = true;
                }
            }
        }
        if changed {
            self.sheet_sets_changed();
        }
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
                    self.sheet_sets_changed();
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
            SheetSetMsg::Poll => {
                self.sync_drawing_locks();
                self.poll_sheet_sets();
                // SSMSHEETSTATUS 2: the status is taken again every SSMPOLLTIME seconds.
                let every = std::time::Duration::from_secs(u64::from(self.sheet_set.settings.poll_time.max(20)));
                if self.sheet_set.settings.sheet_status == 2 && self.sheet_set.status_at.is_none_or(|t| t.elapsed() >= every) {
                    self.refresh_sheet_status();
                }
            }
            SheetSetMsg::BrowseDrawings => {
                return Task::perform(
                    async {
                        crate::sys::file_dialog()
                            .set_title(crate::t!("Select Drawing").as_ref())
                            .add_filter(crate::t!("Drawing (*.dwg)").as_ref(), &["dwg", "DWG"])
                            .pick_files()
                            .await
                            .map(|hs| hs.iter().map(crate::sys::handle_path).collect::<Vec<_>>())
                            .unwrap_or_default()
                    },
                    |p| Message::SheetSet(SheetSetMsg::ImportAdd(p)),
                )
            }
            SheetSetMsg::ImportAdd(paths) => self.import_add(paths),
            SheetSetMsg::Previous => self.rename_step(false),
            SheetSetMsg::Next => self.rename_step(true),
            SheetSetMsg::ViewsByCategory(v) => self.sheet_set.by_category = v,
            SheetSetMsg::NewCategory => self.open_category(None),
            SheetSetMsg::CategoryProperties(id) => self.open_category(Some(id)),
            SheetSetMsg::CategoryRemove(id) => {
                if let Some(db) = self.sheet_set.db_mut() {
                    db.remove(&id);
                }
                self.save_current_sheet_set();
            }
            SheetSetMsg::AddBlocks => {
                return Task::perform(
                    async {
                        crate::sys::file_dialog()
                            .set_title(crate::t!("Select Drawing").as_ref())
                            .add_filter(crate::t!("Drawing (*.dwg)").as_ref(), &["dwg", "DWG", "dwt", "DWT"])
                            .pick_file()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    |p| Message::SheetSet(SheetSetMsg::BlocksPicked(p)),
                )
            }
            SheetSetMsg::BlocksPicked(Some(path)) => self.category_add_blocks(path),
            SheetSetMsg::BlocksPicked(None) | SheetSetMsg::LocationPicked(None) => {}
            SheetSetMsg::AddLocation => {
                return Task::perform(
                    async {
                        crate::sys::file_dialog()
                            .set_title(crate::t!("Browse for Folder").as_ref())
                            .pick_folder()
                            .await
                            .map(|h| crate::sys::handle_path(&h))
                    },
                    |p| Message::SheetSet(SheetSetMsg::LocationPicked(p)),
                )
            }
            SheetSetMsg::LocationPicked(Some(path)) => {
                if let Some(db) = self.sheet_set.db_mut() {
                    db.add_resource(&path.to_string_lossy());
                }
                self.save_current_sheet_set();
            }
            SheetSetMsg::RemoveLocation(id) => {
                if let Some(db) = self.sheet_set.db_mut() {
                    db.remove(&id);
                }
                self.save_current_sheet_set();
            }
            SheetSetMsg::OpenDrawing(path) => return self.update(Message::OpenRecent(PathBuf::from(path))),
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
                let parent_folder = match self.sheet_set.dialog.as_ref() {
                    Some(SsDialog::Form(f)) if f.kind == FormKind::NewSubset => {
                        self.sheet_set.sets.get(f.set).map(|db| sheet_folder_of(db, &f.component)).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                match (field, self.sheet_set.dialog.as_mut()) {
                    (FieldId::Hierarchy, Some(SsDialog::Wizard(w))) => w.hierarchy = v,
                    (FieldId::Hierarchy, Some(SsDialog::Form(f))) => {
                        f.hierarchy = v;
                        f.folder = if v { Path::new(&parent_folder).join(f.title.trim()).to_string_lossy().to_string() } else { parent_folder };
                    }
                    (FieldId::Publish, Some(SsDialog::Form(f))) => f.publish = v,
                    (FieldId::OpenAfter, Some(SsDialog::Form(f))) => f.open_after = v,
                    (FieldId::ImportPrefix, Some(SsDialog::Form(f))) => f.prefix = v,
                    (FieldId::ImportRow(k), Some(SsDialog::Form(f))) => {
                        if let Some(r) = f.rows.get_mut(k) {
                            r.on = v && !r.taken;
                        }
                    }
                    (FieldId::RenameOption(k), Some(SsDialog::Form(f))) => {
                        f.rename[k] = v;
                        // A prefix needs its "Sheet title" option.
                        if !f.rename[0] {
                            f.rename[1] = false;
                        }
                        if !f.rename[2] {
                            f.rename[3] = false;
                        }
                        follow_rename(f);
                    }
                    (FieldId::CategoryBlock(k), Some(SsDialog::Category(c))) => {
                        if let Some(b) = c.blocks.get_mut(k) {
                            b.2 = v;
                        }
                    }
                    (FieldId::Row(r), Some(SsDialog::Properties(p))) => {
                        if let Some(row) = p.rows.get_mut(r) {
                            row.value = if v { "Publish by Sheet 'Include for Publish' Setting" } else { "Do Not Publish Sheets" }.into();
                        }
                    }
                    _ => {}
                }
            }
            SheetSetMsg::Choose(field, k) => match (field, self.sheet_set.dialog.as_mut()) {
                (FieldId::Layout, Some(SsDialog::Template(t))) => t.layout = k,
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
        // On a sheet, New Sheet and Import work on the subset holding it.
        let id = match action {
            MenuAction::NewSheet | MenuAction::ImportLayout
                if db.find(&id).and_then(ComponentKind::of) == Some(ComponentKind::Sheet) =>
            {
                db.parent_of(&id).map_or(id, |p| p.id().to_string())
            }
            _ => id,
        };
        match action {
            MenuAction::Open => return self.open_sheet(&id),
            MenuAction::NewSheet => {
                let template = template_of(db, &id);
                let form = Form {
                    kind: FormKind::NewSheet,
                    set,
                    component: id.clone(),
                    folder: sheet_folder_of(db, &id),
                    drawing: template
                        .as_ref()
                        .map(|t| format!("{} ({})", t.name, t.file_name))
                        .unwrap_or_else(|| crate::t!("(default new drawing)").into_owned()),
                    ..Form::default()
                };
                // A subset that prompts for its template asks for it first.
                if prompts_for_template(db, &id) {
                    let file = template.map(|t| t.file_name).unwrap_or_default();
                    let layouts = crate::io::load_file(Path::new(&file)).map(|d| paper_layouts(&d).into_iter().map(|(n, _)| n).collect()).unwrap_or_default();
                    self.sheet_set.dialog = Some(SsDialog::Template(TemplatePick { form, file, layouts, layout: 0 }));
                } else {
                    self.sheet_set.dialog = Some(SsDialog::Form(form));
                }
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
                    title: format!("New Subset ({n})"),
                    folder: sheet_folder_of(db, &id),
                    publish: true,
                    ..Form::default()
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
                self.sheet_set.dialog = Some(SsDialog::Form(rename_form(db, set, &id)));
                self.active_modal = Some(crate::app::ModalKind::SheetSet);
            }
            MenuAction::Remove => {
                // A sheet asks first; a subset can only be removed while empty.
                if db.find(&id).and_then(ComponentKind::of) == Some(ComponentKind::Sheet) {
                    let sheet = db.find(&id).map(ss::number_and_title).unwrap_or_default();
                    let question = crate::tf!("Are you sure that you want to remove {} Sheet from {} Sheet Set?", sheet, db.name()).into_owned();
                    self.sheet_set.dialog = Some(SsDialog::Confirm(set, id.clone(), question));
                    self.active_modal = Some(crate::app::ModalKind::SheetSet);
                } else if db.find(&id).is_some_and(|el| el.children.iter().all(|c| ComponentKind::of(c).is_none())) {
                    if let Some(db) = self.sheet_set.db_mut() {
                        db.remove(&id);
                    }
                    self.sheet_set.selected = None;
                    self.save_current_sheet_set();
                }
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
            Some(SsDialog::Category(c)) => {
                if field == FieldId::Name {
                    c.name = v;
                }
            }
            Some(SsDialog::Confirm(..)) | Some(SsDialog::Template(_)) => {}
            Some(SsDialog::Form(f)) => {
                let file_follows = f.file_name == format!("{} {}", f.number, f.title).trim();
                match field {
                    FieldId::Number => f.number = v,
                    FieldId::Title => {
                        // A folder hierarchy follows the subset name.
                        if f.kind == FormKind::NewSubset && f.hierarchy {
                            if let Some(parent) = Path::new(&f.folder).parent() {
                                f.folder = parent.join(v.trim()).to_string_lossy().to_string();
                            }
                        }
                        f.title = v
                    }
                    FieldId::FileName => f.file_name = v,
                    FieldId::Folder => f.folder = v,
                    FieldId::LayoutName => f.layout_name = v,
                    _ => {}
                }
                if f.kind == FormKind::Rename {
                    follow_rename(f);
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
        let Some(db) = self.sheet_set.db() else { return };
        let rows = match import_rows(&path, db) {
            Ok(rows) => rows,
            Err(e) => {
                self.command_line.push_error(&format!("{}: {e}", path.display()));
                return;
            }
        };
        self.sheet_set.dialog = Some(SsDialog::Form(Form {
            kind: FormKind::ImportLayout,
            set,
            component: parent,
            rows,
            prefix: true,
            ..Form::default()
        }));
        self.active_modal = Some(crate::app::ModalKind::SheetSet);
    }

    /// Browse for Drawings of the open Import dialog: add their layouts.
    fn import_add(&mut self, paths: Vec<PathBuf>) {
        let Some(SsDialog::Form(f)) = self.sheet_set.dialog.as_ref() else { return };
        let Some(db) = self.sheet_set.sets.get(f.set) else { return };
        let mut added = Vec::new();
        for p in paths {
            match import_rows(&p, db) {
                Ok(rows) => added.extend(rows),
                Err(e) => self.command_line.push_error(&format!("{}: {e}", p.display())),
            }
        }
        // The drawings picked replace the list.
        if let Some(SsDialog::Form(f)) = self.sheet_set.dialog.as_mut() {
            if !added.is_empty() {
                f.rows = added;
            }
        }
    }

    /// Rename & Renumber: number and title into the set, and the layout and
    /// drawing file renamed when they changed.
    fn apply_rename(&mut self, f: &Form) -> Result<(), String> {
        let Some(db) = self.sheet_set.sets.get(f.set) else { return Ok(()) };
        let old = db.layout_reference(&f.component, "Layout").unwrap_or_default();
        let new_layout = f.layout_name.trim().to_string();
        let new_file = if old.file_name.is_empty() || f.file_name.trim().is_empty() {
            old.file_name.clone()
        } else {
            Path::new(&f.folder).join(f.file_name.trim()).to_string_lossy().to_string()
        };
        let file_changed = !old.file_name.is_empty() && ss::path_key(&new_file) != ss::path_key(&old.file_name);
        let layout_changed = !old.name.is_empty() && !new_layout.is_empty() && new_layout != old.name;
        let open_tab = self.tab_showing(Path::new(&old.file_name));
        if file_changed {
            if open_tab.is_some() {
                return Err(crate::t!("The drawing is open; close it to rename its file.").into_owned());
            }
            if Path::new(&new_file).exists() {
                return Err(crate::tf!("{} already exists.", new_file).into_owned());
            }
        }
        if layout_changed {
            match open_tab {
                Some(k) => {
                    self.tabs[k].scene.rename_layout(&old.name, &new_layout);
                    if let Some(mut link) = self.tabs[k].scene.document.sheet_set_data() {
                        link.layout_name = new_layout.clone();
                        self.tabs[k].scene.document.set_sheet_set_data(&link);
                    }
                }
                None => {
                    let mut scene = crate::scene::Scene::new();
                    scene.document = crate::io::load_file(Path::new(&old.file_name)).map_err(|e| e.to_string())?;
                    scene.rename_layout(&old.name, &new_layout);
                    if let Some(mut link) = scene.document.sheet_set_data() {
                        link.layout_name = new_layout.clone();
                        scene.document.set_sheet_set_data(&link);
                    }
                    crate::io::save(&scene.document, Path::new(&old.file_name)).map_err(|e| e.to_string())?;
                }
            }
        }
        if file_changed {
            std::fs::rename(&old.file_name, &new_file).map_err(|e| e.to_string())?;
        }
        let Some(db) = self.sheet_set.sets.get_mut(f.set) else { return Ok(()) };
        if let Some(el) = db.find_mut(&f.component) {
            el.set_prop("Number", f.number.trim());
            el.set_prop("Title", f.title.trim());
        }
        if layout_changed || file_changed {
            db.set_layout_reference(
                &f.component,
                "Layout",
                &LayoutReference {
                    file_name: ss::native_path(&new_file),
                    name: if layout_changed { new_layout } else { old.name },
                    handle: old.handle,
                },
            );
        }
        Ok(())
    }

    /// Rename & Renumber < Previous / Next >: apply, then the neighbouring sheet.
    fn rename_step(&mut self, forward: bool) {
        let Some(SsDialog::Form(f)) = self.sheet_set.dialog.take() else { return };
        if let Err(e) = self.apply_rename(&f) {
            self.sheet_set.dialog = Some(SsDialog::Form(Form { error: Some(e), ..f }));
            return;
        }
        self.sheet_set.current = Some(f.set);
        self.save_current_sheet_set();
        let Some(db) = self.sheet_set.sets.get(f.set) else { return };
        let ids: Vec<String> = db.sheets().iter().map(|s| s.id().to_string()).collect();
        let k = ids.iter().position(|i| *i == f.component).unwrap_or(0);
        let next = if forward { (k + 1).min(ids.len().saturating_sub(1)) } else { k.saturating_sub(1) };
        let mut form = rename_form(db, f.set, &ids[next]);
        form.rename = f.rename;
        follow_rename(&mut form);
        self.sheet_set.selected = Some(ids[next].clone());
        self.sheet_set.dialog = Some(SsDialog::Form(form));
    }

    /// The View Category dialog for a new (`None`) or existing category.
    fn open_category(&mut self, id: Option<String>) {
        let (Some(set), Some(db)) = (self.sheet_set.current, self.sheet_set.db()) else { return };
        let used = id.as_deref().map(|i| db.category_blocks(i));
        let blocks = db
            .callout_blocks()
            .iter()
            .map(|b| {
                let label = format!("{} ({})", b.prop("Name").unwrap_or(""), db.resolve_file(b));
                let on = used.as_ref().is_none_or(|u| u.iter().any(|x| x == b.id()));
                (b.id().to_string(), label, on)
            })
            .collect();
        let name = id.as_deref().and_then(|i| db.find(i)).and_then(|c| c.prop("Name")).unwrap_or("").to_string();
        self.sheet_set.dialog = Some(SsDialog::Category(Category { set, id, name, blocks, error: None }));
        self.active_modal = Some(crate::app::ModalKind::SheetSet);
    }

    /// Add Blocks...: the named blocks of a drawing become callout blocks.
    fn category_add_blocks(&mut self, path: PathBuf) {
        let doc = match crate::io::load_file(&path) {
            Ok(d) => d,
            Err(e) => {
                self.command_line.push_error(&format!("{}: {e}", path.display()));
                return;
            }
        };
        let file = ss::native_path(&path.to_string_lossy());
        let Some(SsDialog::Category(c)) = self.sheet_set.dialog.as_mut() else { return };
        let mut names: Vec<(String, String)> = doc
            .block_records
            .iter()
            .filter(|b| !b.name.starts_with('*') && !b.is_layout())
            .map(|b| (b.name.clone(), format!("{:X}", b.handle.value())))
            .collect();
        names.sort();
        for (name, handle) in names {
            c.blocks.push((format!("new:{file}|{name}|{handle}"), format!("{name} ({file})"), true));
        }
    }

    fn category_ok(&mut self, mut c: Category) {
        let name = c.name.trim().to_string();
        if name.is_empty() {
            c.error = Some(crate::t!("The category name cannot be empty.").into_owned());
            self.sheet_set.dialog = Some(SsDialog::Category(c));
            return;
        }
        self.close_active_modal();
        let Some(db) = self.sheet_set.sets.get_mut(c.set) else { return };
        let mut used = Vec::new();
        for (id, _, on) in &c.blocks {
            let id = match id.strip_prefix("new:") {
                Some(spec) => {
                    let mut p = spec.splitn(3, '|');
                    let (file, block, handle) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
                    match db.add_callout_block(file, block, handle) {
                        Some(id) => id,
                        None => continue,
                    }
                }
                None => id.clone(),
            };
            if *on {
                used.push(id);
            }
        }
        db.set_view_category(c.id.as_deref(), &name, &used);
        self.sheet_set.current = Some(c.set);
        self.save_current_sheet_set();
    }

    /// Model Views: each location folder of the current set and its drawings.
    pub(in crate::app) fn refresh_locations(&mut self) {
        let Some(db) = self.sheet_set.db() else {
            self.sheet_set.locations.clear();
            return;
        };
        self.sheet_set.locations = db
            .sheet_set()
            .named("Resources")
            .map(|r| {
                r.children
                    .iter()
                    .map(|c| {
                        let folder = db.resolve_file(c);
                        let mut files: Vec<String> = std::fs::read_dir(&folder)
                            .map(|d| {
                                d.filter_map(|e| e.ok())
                                    .map(|e| e.file_name().to_string_lossy().to_string())
                                    .filter(|n| n.to_ascii_lowercase().ends_with(".dwg"))
                                    .collect()
                            })
                            .unwrap_or_default();
                        files.sort_by_key(|n| n.to_lowercase());
                        (c.id().to_string(), folder, files)
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    /// SSMSHEETSTATUS: a sheet whose drawing is missing, or open (its `.dwl`
    /// lock file, or a tab of this application).
    pub(in crate::app) fn refresh_sheet_status(&mut self) {
        self.sheet_set.status_at = Some(std::time::Instant::now());
        self.sheet_set.status.clear();
        if self.sheet_set.settings.sheet_status == 0 {
            return;
        }
        let Some(db) = self.sheet_set.db() else { return };
        let mut status = std::collections::HashMap::new();
        for sheet in db.sheets() {
            let Some(r) = db.layout_reference(sheet.id(), "Layout") else { continue };
            let path = Path::new(&r.file_name);
            let state = if !path.exists() {
                Some(SheetStatus::Missing)
            } else if path.with_extension("dwl").exists() {
                Some(SheetStatus::Locked)
            } else {
                None
            };
            if let Some(st) = state {
                status.insert(sheet.id().to_string(), st);
            }
        }
        self.sheet_set.status = status;
    }

    fn properties_template(&mut self, path: PathBuf) {
        let layouts = crate::io::load_file(&path).map(|d| paper_layouts(&d)).unwrap_or_default();
        if let Some(SsDialog::Template(t)) = self.sheet_set.dialog.as_mut() {
            t.file = path.to_string_lossy().to_string();
            t.layouts = layouts.into_iter().map(|(n, _)| n).collect();
            t.layout = 0;
            return;
        }
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
            Some(SsDialog::Confirm(set, id, _)) => {
                self.close_active_modal();
                if let Some(db) = self.sheet_set.sets.get_mut(set) {
                    db.remove(&id);
                }
                self.sheet_set.current = Some(set);
                self.sheet_set.selected = None;
                self.save_current_sheet_set();
                Task::none()
            }
            Some(SsDialog::Template(t)) => {
                let mut form = t.form;
                if let Some(layout) = t.layouts.get(t.layout) {
                    form.drawing = format!("{layout} ({})", t.file);
                    form.template = Some((t.file, layout.clone()));
                }
                self.sheet_set.dialog = Some(SsDialog::Form(form));
                Task::none()
            }
            Some(SsDialog::Category(c)) => {
                self.category_ok(c);
                Task::none()
            }
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
                if let Err(e) = self.apply_rename(&f) {
                    f.error = Some(e);
                    self.sheet_set.dialog = Some(SsDialog::Form(f));
                    return Task::none();
                }
            }
            FormKind::NewSubset => {
                let name = f.title.trim().to_string();
                if name.is_empty() {
                    f.error = Some(crate::t!("The subset name cannot be empty.").into_owned());
                    self.sheet_set.dialog = Some(SsDialog::Form(f));
                    return Task::none();
                }
                let parent_folder = sheet_folder_of(db, &f.component);
                if let Some(sub) = db.add_subset(&f.component, &name, "New subset added") {
                    // Folder hierarchy: the parent's location + the subset name, created now.
                    let folder = if f.hierarchy {
                        let folder = Path::new(&parent_folder).join(&name);
                        let _ = std::fs::create_dir_all(&folder);
                        folder.to_string_lossy().to_string()
                    } else {
                        f.folder.trim().to_string()
                    };
                    if !folder.is_empty() {
                        db.set_file_reference(&sub, "NewSheetLocation", &folder);
                    }
                    if !f.publish {
                        if let Some(el) = db.find_mut(&sub) {
                            el.set_prop_vt("OverrideSheetPublish", 2, "-1");
                        }
                    }
                }
            }
            FormKind::ImportLayout => {
                let chosen: Vec<&ImportRow> = f.rows.iter().filter(|r| r.on && !r.taken).collect();
                if chosen.is_empty() {
                    f.error = Some(crate::t!("The drawing has no layout to import.").into_owned());
                    self.sheet_set.dialog = Some(SsDialog::Form(f));
                    return Task::none();
                }
                for r in chosen {
                    let stem = Path::new(&r.drawing).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let title = if f.prefix { format!("{stem} - {}", r.layout) } else { r.layout.clone() };
                    if let Some(sheet) = db.add_sheet(&f.component, "", &title, "") {
                        db.set_layout_reference(
                            &sheet,
                            "Layout",
                            &LayoutReference { file_name: r.drawing.clone(), name: r.layout.clone(), handle: r.handle.clone() },
                        );
                    }
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
        let template = match f.template.clone() {
            Some((file_name, name)) => Some(LayoutReference { file_name, name, handle: String::new() }),
            None => self.sheet_set.sets.get(f.set).and_then(|db| template_of(db, &f.component)),
        };
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
        // "Open in drawing editor" (off by default) opens the new sheet.
        self.close_active_modal();
        self.sheet_set.current = Some(f.set);
        self.sheet_set.selected = Some(sheet.clone());
        self.sheet_sets_changed();
        if f.open_after {
            return self.open_sheet(&sheet);
        }
        Task::none()
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
            let publish = if el.prop("OverrideSheetPublish").is_some_and(|v| v.trim() == "-1") {
                "Do Not Publish Sheets"
            } else {
                "Publish by Sheet 'Include for Publish' Setting"
            };
            rows.push(row("Subset", "Publish sheets in subset", RowKey::Publish, publish.to_string()));
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
                // The set keeps it as an int (1), a subset as a short (-1).
                if let Some(el) = db.find_mut(&p.component) {
                    el.remove_named("PromptForDwt");
                    match (r.value == "Yes", p.kind) {
                        (true, ComponentKind::SheetSet) => el.set_prop_vt("PromptForDwt", 3, "1"),
                        (true, _) => el.set_prop_vt("PromptForDwt", 2, "-1"),
                        _ => {}
                    }
                }
            }
            RowKey::Publish => {
                if let Some(el) = db.find_mut(&p.component) {
                    if r.value == "Do Not Publish Sheets" {
                        el.set_prop_vt("OverrideSheetPublish", 2, "-1");
                    } else {
                        el.remove_named("OverrideSheetPublish");
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

