//! Sheet Set Manager (SHEETSET): the open sheet sets as a tree of subsets
//! and sheets, the New Sheet Set wizard (NEWSHEETSET), the Sheet Set /
//! Subset / Sheet Properties dialog and the small New Sheet, New Subset,
//! Import Layout and Rename & Renumber forms. The tree is read from the
//! sheet set database (`.dst`) itself on every view.

use crate::app::Message;
use crate::t;
use crate::ui::dock::PanelId;
use crate::ui::style::common::muted_style;
use crate::ui::style::form::{button_style, dialog_button, field_style};
use crate::ui::window::pdf_dialogs::{accent_text, card_style};
use codec::sheet_set::{self as ss, ComponentKind, Element as SsElement, SheetSetDatabase};
use iced::widget::{
    button, checkbox, column, container, mouse_area, pick_list, row, scrollable, text, text_input, Space,
};
use iced::{Background, Border, Element, Fill, Length, Theme};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const SET_ICON: &[u8] = include_bytes!("../../../assets/icons/sheetset.svg");

/// SSMAUTOOPEN, SSLOCATE, SSMPOLLTIME and SSMSHEETSTATUS: profile settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SheetSetSettings {
    pub auto_open: u8,
    pub locate: u8,
    pub poll_time: u16,
    pub sheet_status: u8,
}

impl Default for SheetSetSettings {
    fn default() -> Self {
        Self { auto_open: 1, locate: 1, poll_time: 60, sheet_status: 2 }
    }
}

/// The palette's three pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SsmTab {
    #[default]
    SheetList,
    SheetViews,
    ModelViews,
}

/// The palette and the open sheet sets.
#[derive(Debug, Default)]
pub struct SheetSetManager {
    pub show: bool,
    pub settings: SheetSetSettings,
    /// Open sheet sets (each read from its `.dst`).
    pub sets: Vec<SheetSetDatabase>,
    /// The set the palette shows.
    pub current: Option<usize>,
    pub tab: SsmTab,
    /// Folded subset / set ids.
    pub collapsed: HashSet<String>,
    /// Highlighted component id.
    pub selected: Option<String>,
    /// SSFOUND: the `.dst` the last opened sheet drawing named, when found.
    pub found: String,
    /// The open dialog (shown in the `SheetSet` modal).
    pub dialog: Option<SsDialog>,
}

impl SheetSetManager {
    pub fn db(&self) -> Option<&SheetSetDatabase> {
        self.sets.get(self.current?)
    }

    pub fn db_mut(&mut self) -> Option<&mut SheetSetDatabase> {
        self.sets.get_mut(self.current?)
    }
}

/// Right-click commands of a tree row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    NewSheet,
    NewSubset,
    ImportLayout,
    Rename,
    Remove,
    Properties,
    Close,
}

#[derive(Debug, Clone)]
pub enum SheetSetMsg {
    /// The set combo: a set by index, or `None` for "Open...".
    PickSet(Option<usize>),
    Tab(SsmTab),
    Expand(String),
    Select(String),
    /// Double-click: open the sheet's drawing at its layout.
    Activate(String),
    Menu(String, MenuAction),
    Refresh,
    /// A `.dst` picked by Open (or given by automation).
    OpenPicked(Option<std::path::PathBuf>),
    /// A drawing picked by Import Layout as Sheet.
    ImportPicked(Option<std::path::PathBuf>),
    /// A folder picked for a dialog field.
    FolderPicked(FieldId, Option<std::path::PathBuf>),
    /// A template drawing picked for a sheet creation template.
    TemplatePicked(Option<std::path::PathBuf>),
    Browse(FieldId),
    BrowseTemplate,
    /// Edit a text field of the open dialog.
    Input(FieldId, String),
    Toggle(FieldId, bool),
    Choose(FieldId, usize),
    Wizard(WizardMsg),
    /// Open the Properties dialog from the wizard.
    WizardProperties,
    AddCustom,
    AddCustomOk,
    AddCustomCancel,
    /// A custom property row's value.
    CustomValue(usize, String),
    Ok,
    Cancel,
    Help,
}

#[derive(Debug, Clone)]
pub enum WizardMsg {
    Step(u8),
    Existing(bool),
    /// Toggle one drawing layout of the "existing drawings" page.
    LayoutOn(usize, bool),
    RemoveFolder(usize),
}

/// Text fields, toggles and lists of the dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldId {
    /// A Properties row by index.
    Row(usize),
    Name,
    Description,
    Folder,
    Hierarchy,
    Number,
    Title,
    FileName,
    /// The layout list of Import Layout as Sheet.
    Layout,
    /// Add Custom Property: name, default value, owner (0 set, 1 sheet).
    CustomName,
    CustomDefault,
    CustomOwner,
    /// A wizard "existing drawings" folder to add.
    AddFolder,
    /// Prompt for template (Properties).
    PromptTemplate,
}

/// What a Properties row edits.
#[derive(Debug, Clone, PartialEq)]
pub enum RowKey {
    /// An `AcSmProp` of the component.
    Prop(&'static str),
    /// A file reference property (`NewSheetLocation`).
    Folder(&'static str),
    /// The sheet creation template (`DefDwtLayout`): the value shows
    /// `Layout (file)`; `template` holds the picked drawing and layout.
    Template,
    /// Yes / No: prompt for template.
    PromptTemplate,
    /// Shown only.
    ReadOnly,
}

#[derive(Debug, Clone)]
pub struct PropRow {
    pub group: &'static str,
    pub label: &'static str,
    pub key: RowKey,
    pub value: String,
}

/// The Sheet Set / Subset / Sheet Properties dialog.
#[derive(Debug, Clone)]
pub struct Properties {
    /// `None` edits the wizard's draft set.
    pub set: Option<usize>,
    pub component: String,
    pub kind: ComponentKind,
    pub title: String,
    pub rows: Vec<PropRow>,
    /// Name, value, flags.
    pub custom: Vec<(String, String, i32)>,
    /// Add Custom Property being filled: name, default value, owner is sheet.
    pub adding: Option<(String, String, bool)>,
    /// The template picked in this dialog: drawing and layout.
    pub template: Option<(String, String)>,
    /// Back to the wizard on OK / Cancel.
    pub wizard: Option<Box<Wizard>>,
    pub error: Option<String>,
}

/// One layout found by the "existing drawings" wizard page.
#[derive(Debug, Clone)]
pub struct FoundLayout {
    pub folder: String,
    pub file: String,
    pub layout: String,
    pub handle: String,
    pub on: bool,
}

/// NEWSHEETSET.
#[derive(Debug, Clone)]
pub struct Wizard {
    /// 0 Begin, 1 Sheet Set Details, 2 Choose Layouts (existing drawings), 3 Confirm.
    pub step: u8,
    pub existing: bool,
    pub name: String,
    pub description: String,
    pub folder: String,
    pub hierarchy: bool,
    pub folders: Vec<String>,
    pub layouts: Vec<FoundLayout>,
    /// The set being built: name, description, properties and custom
    /// properties go into it; Finish writes it.
    pub draft: SheetSetDatabase,
    pub error: Option<String>,
}

/// New Sheet, New Subset, Rename & Renumber and Import Layout as Sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    NewSheet,
    NewSubset,
    Rename,
    ImportLayout,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub kind: FormKind,
    pub set: usize,
    /// Parent (new sheet / subset / import) or the sheet / subset renamed.
    pub component: String,
    pub number: String,
    pub title: String,
    pub file_name: String,
    pub folder: String,
    /// Import: the drawing and its layouts (name, handle).
    pub drawing: String,
    pub layouts: Vec<(String, String)>,
    pub layout: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SsDialog {
    Wizard(Wizard),
    Properties(Properties),
    Form(Form),
}

impl SsDialog {
    pub fn title(&self) -> String {
        match self {
            SsDialog::Wizard(_) => t!("Create Sheet Set").into_owned(),
            SsDialog::Properties(p) => p.title.clone(),
            SsDialog::Form(f) => match f.kind {
                FormKind::NewSheet => t!("New Sheet").into_owned(),
                FormKind::NewSubset => t!("Subset Properties").into_owned(),
                FormKind::Rename => t!("Rename & Renumber Sheet").into_owned(),
                FormKind::ImportLayout => t!("Import Layout as Sheet").into_owned(),
            },
        }
    }
}

fn msg(m: SheetSetMsg) -> Message {
    Message::SheetSet(m)
}

// ── palette ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pick(Option<usize>, String);

impl std::fmt::Display for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}

fn menu_entry(label: String, m: Option<Message>) -> Element<'static, Message> {
    button(text(label).size(12))
        .width(Fill)
        .padding([5, 12])
        .on_press_maybe(m)
        .style(|theme: &Theme, status| {
            let mut s = button::text(theme, status);
            if matches!(status, button::Status::Hovered) {
                s.background = Some(Background::Color(theme.palette().primary.base.color));
                s.text_color = theme.palette().primary.base.text;
            }
            s
        })
        .into()
}

fn menu_separator() -> Element<'static, Message> {
    container(Space::new())
        .height(1)
        .width(Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.palette().background.neutral.color)),
            ..Default::default()
        })
        .into()
}

/// A row's right-click menu.
fn row_menu(id: String, kind: ComponentKind) -> Element<'static, Message> {
    let entry = |label: &str, action: MenuAction, on: bool| {
        menu_entry(t!(label).into_owned(), on.then(|| msg(SheetSetMsg::Menu(id.clone(), action))))
    };
    let sheet = kind == ComponentKind::Sheet;
    let mut items = vec![
        entry("Open", MenuAction::Open, sheet),
        menu_separator(),
        entry("New Sheet...", MenuAction::NewSheet, !sheet),
        entry("New Subset...", MenuAction::NewSubset, !sheet),
        entry("Import Layout as Sheet...", MenuAction::ImportLayout, !sheet),
        menu_separator(),
        entry("Rename & Renumber...", MenuAction::Rename, sheet),
        entry(if kind == ComponentKind::Subset { "Remove Subset" } else { "Remove Sheet" }, MenuAction::Remove, kind != ComponentKind::SheetSet),
    ];
    if kind == ComponentKind::SheetSet {
        items.push(entry("Close Sheet Set", MenuAction::Close, true));
    }
    items.push(menu_separator());
    items.push(entry("Properties...", MenuAction::Properties, true));
    container(iced::widget::Column::with_children(items).spacing(1))
        .padding(4)
        .width(Length::Fixed(220.0))
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.palette().background.weak.color)),
            border: Border { color: theme.palette().background.neutral.color, width: 1.0, radius: 4.0.into() },
            ..Default::default()
        })
        .into()
}

fn tree_row<'a>(
    state: &SheetSetManager,
    el: &SsElement,
    depth: u16,
    icon: &'static [u8],
    label: String,
    fold: Option<bool>,
) -> Element<'a, Message> {
    let id = el.id().to_string();
    let kind = ComponentKind::of(el).unwrap_or(ComponentKind::Sheet);
    let arrow: Element<'a, Message> = match fold {
        Some(open) => button(if open {
            crate::ui::icons::themed_arrow_down(10.0)
        } else {
            crate::ui::icons::themed_arrow_right(10.0)
        })
        .on_press(msg(SheetSetMsg::Expand(id.clone())))
        .style(button::text)
        .padding(2)
        .into(),
        None => Space::new().width(14).into(),
    };
    let selected = state.selected.as_deref() == Some(id.as_str());
    let cells = row![
        Space::new().width(Length::Fixed(f32::from(depth) * 16.0)),
        arrow,
        crate::ui::icons::semantic(icon, 14.0),
        text(label).size(12).width(Fill),
    ]
    .spacing(5)
    .align_y(iced::Center);
    let area = mouse_area(container(cells).width(Fill).padding([3, 6]).style(move |theme: &Theme| container::Style {
        background: selected.then(|| Background::Color(theme.palette().primary.weak.color)),
        text_color: selected.then(|| theme.palette().primary.weak.text),
        ..Default::default()
    }))
    .on_press(msg(SheetSetMsg::Select(id.clone())))
    .on_double_click(msg(SheetSetMsg::Activate(id.clone())));
    iced_aw::ContextMenu::new(area, move || row_menu(id.clone(), kind)).into()
}

fn push_tree<'a>(
    state: &SheetSetManager,
    el: &SsElement,
    depth: u16,
    out: &mut Vec<Element<'a, Message>>,
) {
    for c in &el.children {
        match ComponentKind::of(c) {
            Some(ComponentKind::Subset) => {
                let open = !state.collapsed.contains(c.id());
                out.push(tree_row(state, c, depth, crate::ui::icons::FOLDER_OPEN, c.prop("Name").unwrap_or("").to_string(), Some(open)));
                if open {
                    push_tree(state, c, depth + 1, out);
                }
            }
            Some(ComponentKind::Sheet) => {
                out.push(tree_row(state, c, depth, crate::ui::icons::DOC, ss::number_and_title(c), None));
            }
            _ => {}
        }
    }
}

/// Sheet views of the set: each sheet's `AcSmSheetView` children.
fn sheet_views(db: &SheetSetDatabase) -> Vec<String> {
    db.sheets()
        .into_iter()
        .flat_map(|sheet| {
            sheet
                .named("SheetViews")
                .map(|v| v.children.iter().filter(|c| c.name == "AcSmSheetView").collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    let number = v.prop("Number").unwrap_or("");
                    let title = v.prop("Title").unwrap_or("");
                    if number.is_empty() { title.to_string() } else { format!("{number} - {title}") }
                })
        })
        .collect()
}

/// Model view locations (`Resources`) of the set.
fn model_views(db: &SheetSetDatabase) -> Vec<String> {
    db.sheet_set()
        .named("Resources")
        .map(|r| r.children.iter().map(|c| db.resolve_file(c)).collect())
        .unwrap_or_default()
}

fn heading(label: &str) -> Element<'static, Message> {
    text(t!(label).into_owned())
        .size(11)
        .font(iced::Font { weight: iced::font::Weight::Bold, ..iced::Font::DEFAULT })
        .style(|theme: &Theme| text::Style { color: Some(theme.palette().primary.base.color) })
        .width(Fill)
        .into()
}

pub fn view<'a>(state: &'a SheetSetManager, width: f32, auto_collapse: bool) -> Element<'a, Message> {
    let title_bar =
        crate::ui::dock::title_bar(PanelId::SheetSetManager, t!("Sheet Set Manager").into_owned(), auto_collapse);
    let mut picks: Vec<Pick> =
        state.sets.iter().enumerate().map(|(i, db)| Pick(Some(i), db.name().to_string())).collect();
    picks.push(Pick(None, t!("Open...").into_owned()));
    let current = state.current.and_then(|i| picks.get(i).cloned());
    let combo = pick_list(current, picks, |p: &Pick| p.1.clone())
        .placeholder(t!("Open...").into_owned())
        .on_select(|p: Pick| msg(SheetSetMsg::PickSet(p.0)))
        .text_size(12)
        .padding([5, 8])
        .width(Fill);
    let tab = |label: &str, t: SsmTab| {
        button(text(t!(label).into_owned()).size(11).center().width(Fill))
            .on_press(msg(SheetSetMsg::Tab(t)))
            .style(button_style(state.tab == t))
            .padding([5, 4])
            .width(Fill)
    };
    let tabs = row![
        tab("Sheet List", SsmTab::SheetList),
        tab("Sheet Views", SsmTab::SheetViews),
        tab("Model Views", SsmTab::ModelViews),
    ]
    .spacing(2);

    let body: Element<'a, Message> = match (state.db(), state.tab) {
        (None, _) => text(t!("Open a sheet set or create one with NEWSHEETSET.").into_owned())
            .size(12)
            .style(muted_style)
            .into(),
        (Some(db), SsmTab::SheetList) => {
            let set = db.sheet_set();
            let open = !state.collapsed.contains(set.id());
            let mut rows = vec![tree_row(state, set, 0, SET_ICON, db.name().to_string(), Some(open))];
            if open {
                push_tree(state, set, 1, &mut rows);
            }
            let header = row![
                heading("Sheets"),
                button(text("↻").size(13))
                    .on_press(msg(SheetSetMsg::Refresh))
                    .style(button::subtle)
                    .padding([2, 4]),
            ]
            .align_y(iced::Center);
            column![header, scrollable(iced::widget::Column::with_children(rows).spacing(1)).height(Fill)]
                .spacing(6)
                .into()
        }
        (Some(db), SsmTab::SheetViews) => {
            let views = sheet_views(db);
            let list: Element<'a, Message> = if views.is_empty() {
                text(t!("No sheet views.").into_owned()).size(12).style(muted_style).into()
            } else {
                iced::widget::Column::with_children(views.into_iter().map(|v| text(v).size(12).into())).spacing(3).into()
            };
            column![heading("Views by sheet"), list].spacing(6).into()
        }
        (Some(db), SsmTab::ModelViews) => {
            let locations = model_views(db);
            let list: Element<'a, Message> = if locations.is_empty() {
                text(t!("No locations.").into_owned()).size(12).style(muted_style).into()
            } else {
                iced::widget::Column::with_children(locations.into_iter().map(|v| text(v).size(12).into()))
                    .spacing(3)
                    .into()
            };
            column![heading("Locations"), list].spacing(6).into()
        }
    };
    let card = container(body).padding(6).width(Fill).height(Fill).style(|theme: &Theme| container::Style {
        background: Some(Background::Color(theme.palette().background.weak.color)),
        border: Border { radius: 4.0.into(), ..Default::default() },
        ..Default::default()
    });
    let hint = text(t!("Double-click opens a sheet. Right-click for more commands.").into_owned())
        .size(10)
        .style(muted_style);
    crate::ui::dock::frame(column![title_bar, combo, tabs, card, hint].spacing(6), width)
}

// ── dialogs ──────────────────────────────────────────────────────────────────

fn card<'a>(title: &str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(column![text(t!(title).to_uppercase()).size(10).style(accent_text), content.into()].spacing(8))
        .padding([10, 12])
        .width(Fill)
        .style(card_style)
        .into()
}

fn input<'a>(value: &str, field: FieldId) -> Element<'a, Message> {
    text_input("", value)
        .size(12)
        .padding([5, 8])
        .style(field_style)
        .on_input(move |v| msg(SheetSetMsg::Input(field, v)))
        .into()
}

fn labeled<'a>(label: &str, control: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    row![text(t!(label).into_owned()).size(12).width(Length::Fixed(170.0)), control.into()]
        .spacing(8)
        .align_y(iced::Center)
        .into()
}

fn browse<'a>(m: SheetSetMsg) -> Element<'a, Message> {
    button(text("...").size(12)).on_press(msg(m)).style(button_style(false)).padding([5, 10]).into()
}

fn error_band<'a>(error: &Option<String>) -> Option<Element<'a, Message>> {
    error.as_ref().map(|e| {
        container(text(e.clone()).size(12))
            .padding([6, 10])
            .width(Fill)
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(theme.palette().danger.weak.color)),
                text_color: Some(theme.palette().danger.weak.text),
                border: Border { radius: 4.0.into(), ..Default::default() },
                ..Default::default()
            })
            .into()
    })
}

fn footer<'a>(left: Vec<Element<'a, Message>>, right: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut r = row![button(text("?").size(12)).on_press(msg(SheetSetMsg::Help)).style(button_style(false)).padding([5, 11])]
        .spacing(6)
        .align_y(iced::Center);
    for e in left {
        r = r.push(e);
    }
    r = r.push(Space::new().width(Fill));
    for e in right {
        r = r.push(e);
    }
    r.into()
}

fn wizard_view<'a>(w: &'a Wizard) -> Element<'a, Message> {
    let steps: Vec<(u8, &str)> = if w.existing {
        vec![(0, "Begin"), (1, "Details"), (2, "Choose Layouts"), (3, "Confirm")]
    } else {
        vec![(0, "Begin"), (1, "Details"), (3, "Confirm")]
    };
    let chips = iced::widget::Row::with_children(steps.iter().enumerate().map(|(n, (s, label))| {
        button(text(format!("{} {}", n + 1, t!(*label))).size(12))
            .on_press(msg(SheetSetMsg::Wizard(WizardMsg::Step(*s))))
            .style(button_style(w.step == *s))
            .padding([4, 10])
            .into()
    }))
    .spacing(6);
    let page: Element<'a, Message> = match w.step {
        0 => card(
            "Create a sheet set using",
            column![
                checkbox(!w.existing)
                    .label(t!("An empty sheet set").into_owned())
                    .text_size(12)
                    .size(14)
                    .on_toggle(|_| msg(SheetSetMsg::Wizard(WizardMsg::Existing(false)))),
                checkbox(w.existing)
                    .label(t!("Existing drawings").into_owned())
                    .text_size(12)
                    .size(14)
                    .on_toggle(|_| msg(SheetSetMsg::Wizard(WizardMsg::Existing(true)))),
                text(
                    t!(if w.existing {
                        "Lets you specify one or more folders containing drawings. The layouts from these drawings can be automatically imported into the sheet set."
                    } else {
                        "Creates a sheet set with no subsets or sheets."
                    })
                    .into_owned()
                )
                .size(11)
                .style(muted_style),
            ]
            .spacing(8),
        ),
        1 => card(
            "Sheet Set",
            column![
                labeled("Name of new sheet set:", input(&w.name, FieldId::Name)),
                labeled("Description (optional):", input(&w.description, FieldId::Description)),
                labeled(
                    "Store sheet set data file (.dst) here:",
                    row![input(&w.folder, FieldId::Folder), browse(SheetSetMsg::Browse(FieldId::Folder))].spacing(6)
                ),
                checkbox(w.hierarchy)
                    .label(t!("Create a folder hierarchy based on subsets").into_owned())
                    .text_size(12)
                    .size(14)
                    .on_toggle(|v| msg(SheetSetMsg::Toggle(FieldId::Hierarchy, v))),
                row![
                    Space::new().width(Fill),
                    button(text(t!("Sheet Set Properties...").into_owned()).size(12))
                        .on_press(msg(SheetSetMsg::WizardProperties))
                        .style(button_style(false))
                        .padding([5, 12]),
                ],
            ]
            .spacing(8),
        ),
        2 => {
            let rows = w.layouts.iter().enumerate().map(|(i, l)| {
                checkbox(l.on)
                    .label(format!("{} — {}", l.file, l.layout))
                    .text_size(12)
                    .size(14)
                    .on_toggle(move |v| msg(SheetSetMsg::Wizard(WizardMsg::LayoutOn(i, v))))
                    .into()
            });
            let folders = w.folders.iter().enumerate().map(|(i, f)| {
                row![
                    text(f.clone()).size(12).width(Fill),
                    button(text("×").size(12))
                        .on_press(msg(SheetSetMsg::Wizard(WizardMsg::RemoveFolder(i))))
                        .style(button::text)
                        .padding([0, 6]),
                ]
                .into()
            });
            card(
                "Choose Layouts",
                column![
                    row![
                        text(t!("Select folders containing drawings. Layouts in the drawings can be added to the sheet set.").into_owned())
                            .size(11)
                            .style(muted_style)
                            .width(Fill),
                        button(text(t!("Browse...").into_owned()).size(12))
                            .on_press(msg(SheetSetMsg::Browse(FieldId::AddFolder)))
                            .style(button_style(false))
                            .padding([5, 12]),
                    ]
                    .spacing(8)
                    .align_y(iced::Center),
                    iced::widget::Column::with_children(folders.collect::<Vec<_>>()).spacing(2),
                    container(scrollable(iced::widget::Column::with_children(rows.collect::<Vec<_>>()).spacing(3)))
                        .height(Length::Fixed(180.0)),
                ]
                .spacing(8),
            )
        }
        _ => {
            let mut lines = vec![
                format!("{}: {}", t!("Sheet Set"), w.name),
                format!("{}: {}", t!("Sheet set data file"), wizard_dst_path(w)),
                format!("{}: {}", t!("Description"), w.description),
            ];
            if w.existing {
                let n = w.layouts.iter().filter(|l| l.on).count();
                lines.push(format!("{}: {n}", t!("Sheets to import")));
            }
            for (name, value, _) in ss::custom_properties(w.draft.sheet_set()) {
                lines.push(format!("{name}: {value}"));
            }
            card(
                "Confirm",
                iced::widget::Column::with_children(lines.into_iter().map(|l| text(l).size(12).into())).spacing(4),
            )
        }
    };
    let next_step = match (w.step, w.existing) {
        (0, _) => Some(1),
        (1, true) => Some(2),
        (1, false) | (2, _) => Some(3),
        _ => None,
    };
    let back_step = match (w.step, w.existing) {
        (3, true) => Some(2),
        (3, false) | (2, _) => Some(1),
        (1, _) => Some(0),
        _ => None,
    };
    let back = button(text(format!("‹ {}", t!("Back"))).size(12))
        .on_press_maybe(back_step.map(|s| msg(SheetSetMsg::Wizard(WizardMsg::Step(s)))))
        .style(button_style(false))
        .padding([6, 14]);
    let next: Element<'a, Message> = match next_step {
        Some(s) => button(text(format!("{} ›", t!("Next"))).size(12))
            .on_press(msg(SheetSetMsg::Wizard(WizardMsg::Step(s))))
            .style(button_style(true))
            .padding([6, 14])
            .into(),
        None => dialog_button(t!("Finish"), msg(SheetSetMsg::Ok), true).into(),
    };
    let mut col = column![chips, page].spacing(10);
    if let Some(e) = error_band(&w.error) {
        col = col.push(e);
    }
    col.push(footer(vec![], vec![back.into(), next, dialog_button(t!("Cancel"), msg(SheetSetMsg::Cancel), false).into()]))
        .into()
}

/// Where the wizard writes the `.dst`.
pub fn wizard_dst_path(w: &Wizard) -> String {
    ss::native_path(&std::path::Path::new(&w.folder).join(format!("{}.dst", w.name.trim())).to_string_lossy())
}

fn properties_view<'a>(p: &'a Properties) -> Element<'a, Message> {
    let mut groups: Vec<Element<'a, Message>> = Vec::new();
    let mut current: Option<&'static str> = None;
    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    let flush = |group: Option<&'static str>, rows: &mut Vec<Element<'a, Message>>, groups: &mut Vec<Element<'a, Message>>| {
        if let Some(g) = group {
            groups.push(card(g, iced::widget::Column::with_children(std::mem::take(rows)).spacing(6)));
        }
    };
    for (i, r) in p.rows.iter().enumerate() {
        if current != Some(r.group) {
            flush(current, &mut rows, &mut groups);
            current = Some(r.group);
        }
        let control: Element<'a, Message> = match r.key {
            RowKey::ReadOnly => container(text(r.value.clone()).size(12).style(muted_style))
                .padding([5, 8])
                .width(Fill)
                .into(),
            RowKey::Folder(_) => row![input(&r.value, FieldId::Row(i)), browse(SheetSetMsg::Browse(FieldId::Row(i)))]
                .spacing(6)
                .into(),
            RowKey::Template => row![
                container(text(r.value.clone()).size(12)).padding([5, 8]).width(Fill),
                browse(SheetSetMsg::BrowseTemplate)
            ]
            .spacing(6)
            .align_y(iced::Center)
            .into(),
            RowKey::PromptTemplate => {
                let yes = r.value == "Yes";
                let opts = vec![t!("Yes").into_owned(), t!("No").into_owned()];
                pick_list(Some(opts[if yes { 0 } else { 1 }].clone()), opts.clone(), |s: &String| s.clone())
                    .on_select(move |s: String| msg(SheetSetMsg::Choose(FieldId::Row(i), usize::from(s != t!("Yes")))))
                    .text_size(12)
                    .padding([5, 8])
                    .width(Fill)
                    .into()
            }
            RowKey::Prop(_) => input(&r.value, FieldId::Row(i)),
        };
        rows.push(labeled(r.label, control));
    }
    flush(current, &mut rows, &mut groups);
    if p.kind != ComponentKind::Subset {
        let owner = |f: i32| if f & ss::CUSTOM_SHEET_PROP != 0 { t!("Sheet") } else { t!("Sheet Set") };
        let mut custom: Vec<Element<'a, Message>> = p
            .custom
            .iter()
            .enumerate()
            .map(|(i, (name, value, flags))| {
                let label = if p.kind == ComponentKind::SheetSet { format!("{name} ({})", owner(*flags)) } else { name.clone() };
                row![
                    text(label).size(12).width(Length::Fixed(170.0)),
                    text_input("", value)
                        .size(12)
                        .padding([5, 8])
                        .style(field_style)
                        .on_input(move |v| msg(SheetSetMsg::CustomValue(i, v))),
                ]
                .spacing(8)
                .align_y(iced::Center)
                .into()
            })
            .collect();
        if p.kind == ComponentKind::SheetSet {
            match &p.adding {
                Some((name, value, sheet)) => custom.push(
                    container(
                        column![
                            labeled("Name:", input(name, FieldId::CustomName)),
                            labeled("Default value:", input(value, FieldId::CustomDefault)),
                            labeled(
                                "Owner",
                                row![
                                    checkbox(!*sheet)
                                        .label(t!("Sheet Set").into_owned())
                                        .text_size(12)
                                        .size(14)
                                        .on_toggle(|_| msg(SheetSetMsg::Choose(FieldId::CustomOwner, 0))),
                                    checkbox(*sheet)
                                        .label(t!("Sheet").into_owned())
                                        .text_size(12)
                                        .size(14)
                                        .on_toggle(|_| msg(SheetSetMsg::Choose(FieldId::CustomOwner, 1))),
                                ]
                                .spacing(12)
                            ),
                            row![
                                Space::new().width(Fill),
                                dialog_button(t!("Cancel"), msg(SheetSetMsg::AddCustomCancel), false),
                                dialog_button(t!("OK"), msg(SheetSetMsg::AddCustomOk), true),
                            ]
                            .spacing(6),
                        ]
                        .spacing(6),
                    )
                    .padding(8)
                    .style(|theme: &Theme| container::Style {
                        border: Border { color: theme.palette().background.neutral.color, width: 1.0, radius: 4.0.into() },
                        ..Default::default()
                    })
                    .into(),
                ),
                None => custom.push(
                    row![
                        Space::new().width(Fill),
                        button(text(t!("Add Custom Property...").into_owned()).size(12))
                            .on_press(msg(SheetSetMsg::AddCustom))
                            .style(button_style(false))
                            .padding([5, 12]),
                    ]
                    .into(),
                ),
            }
        }
        groups.push(card("Custom Properties", iced::widget::Column::with_children(custom).spacing(6)));
    }
    let mut col = column![scrollable(iced::widget::Column::with_children(groups).spacing(10)).height(Fill)].spacing(10);
    if let Some(e) = error_band(&p.error) {
        col = col.push(e);
    }
    col.push(footer(
        vec![],
        vec![
            dialog_button(t!("Cancel"), msg(SheetSetMsg::Cancel), false).into(),
            dialog_button(t!("OK"), msg(SheetSetMsg::Ok), true).into(),
        ],
    ))
    .into()
}

fn form_view<'a>(f: &'a Form) -> Element<'a, Message> {
    let body: Element<'a, Message> = match f.kind {
        FormKind::NewSheet => column![
            labeled("Number", input(&f.number, FieldId::Number)),
            labeled("Sheet title", input(&f.title, FieldId::Title)),
            labeled("File name", input(&f.file_name, FieldId::FileName)),
            labeled("Folder path", container(text(f.folder.clone()).size(12).style(muted_style)).padding([5, 8])),
            labeled("Sheet template", container(text(f.drawing.clone()).size(12).style(muted_style)).padding([5, 8])),
        ]
        .spacing(8)
        .into(),
        FormKind::NewSubset => column![
            labeled("Subset name", input(&f.title, FieldId::Title)),
            labeled(
                "New sheet location",
                row![input(&f.folder, FieldId::Folder), browse(SheetSetMsg::Browse(FieldId::Folder))].spacing(6)
            ),
        ]
        .spacing(8)
        .into(),
        FormKind::Rename => column![
            labeled("Number", input(&f.number, FieldId::Number)),
            labeled("Sheet title", input(&f.title, FieldId::Title)),
        ]
        .spacing(8)
        .into(),
        FormKind::ImportLayout => {
            let rows = f.layouts.iter().enumerate().map(|(i, (name, _))| {
                button(text(name.clone()).size(12))
                    .on_press(msg(SheetSetMsg::Choose(FieldId::Layout, i)))
                    .style(button_style(f.layout == i))
                    .padding([3, 8])
                    .width(Fill)
                    .into()
            });
            column![
                labeled("Drawing", container(text(f.drawing.clone()).size(12).style(muted_style)).padding([5, 8])),
                text(t!("Layouts").into_owned()).size(12),
                container(scrollable(iced::widget::Column::with_children(rows.collect::<Vec<_>>()).spacing(1)))
                    .height(Length::Fixed(160.0)),
            ]
            .spacing(8)
            .into()
        }
    };
    let mut col = column![card(
        match f.kind {
            FormKind::NewSheet => "Sheet",
            FormKind::NewSubset => "Subset",
            FormKind::Rename => "Sheet",
            FormKind::ImportLayout => "Import Layout as Sheet",
        },
        body
    )]
    .spacing(10);
    if let Some(e) = error_band(&f.error) {
        col = col.push(e);
    }
    col.push(footer(
        vec![],
        vec![
            dialog_button(t!("Cancel"), msg(SheetSetMsg::Cancel), false).into(),
            dialog_button(t!("OK"), msg(SheetSetMsg::Ok), true).into(),
        ],
    ))
    .into()
}

pub fn dialog_view<'a>(state: &'a SheetSetManager, sizing: crate::ui::modal::ModalSizing) -> Element<'a, Message> {
    let body = match state.dialog.as_ref() {
        Some(SsDialog::Wizard(w)) => wizard_view(w),
        Some(SsDialog::Properties(p)) => properties_view(p),
        Some(SsDialog::Form(f)) => form_view(f),
        None => Space::new().into(),
    };
    container(body).padding([10, 12]).width(sizing.width).into()
}
