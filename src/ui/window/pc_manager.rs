//! Point Cloud Manager (POINTCLOUDMANAGER): the regions and scans of the
//! drawing's point clouds as a tree, each with a switch that shows or hides
//! its points. The switches are the cloud's own visibility extended data, so
//! the tree is read straight from the document on every view.

use crate::app::Message;
use crate::scene::model::point_cloud::{self, CLOUD_OFF, UNASSIGNED_OFF};
use crate::ui::dock::PanelId;
use codec::entities::ExtendedEntityData;
use codec::{CadDocument, EntityType, Handle};
use iced::widget::{button, checkbox, column, container, mouse_area, row, scrollable, text, text_input, tooltip, Space};
use iced::{Background, Border, Element, Fill, Length, Theme};
use std::collections::HashSet;

const CLOUD_ICON: &[u8] = include_bytes!("../../../assets/icons/pc_attach.svg");
const REGION_ICON: &[u8] = include_bytes!("../../../assets/icons/region.svg");
const UNASSIGNED_ICON: &[u8] = include_bytes!("../../../assets/icons/revcloud.svg");

/// Palette state that is not in the drawing: shown, folded nodes, the
/// highlighted row and the search text.
#[derive(Debug, Default)]
pub struct PcManager {
    pub show: bool,
    pub collapsed: HashSet<String>,
    pub selected: Option<String>,
    pub search: String,
}

/// A row with a switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Cloud,
    Unassigned,
    Scans,
    Scan(String),
}

impl Row {
    /// `cloud`, `unassigned`, `scans` or `scan:<name>` (automation).
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "cloud" => Some(Self::Cloud),
            "unassigned" => Some(Self::Unassigned),
            "scans" => Some(Self::Scans),
            _ => text.strip_prefix("scan:").map(|name| Self::Scan(name.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub enum PcManagerMsg {
    Toggle(Handle, Row),
    Expand(String),
    Select(String),
    Search(String),
    CollapseAll,
    ExpandAll,
}

/// One cloud of the tree.
pub struct Cloud {
    pub handle: Handle,
    pub name: String,
    pub hidden: Vec<String>,
    pub scans: Vec<String>,
}

/// The drawing's point clouds; only the selected ones when any is selected.
pub fn clouds(document: &CadDocument, selected: &[Handle]) -> Vec<Cloud> {
    let all: Vec<Cloud> = document
        .entities()
        .filter_map(|entity| {
            let EntityType::Extended(extended) = entity else {
                return None;
            };
            let ExtendedEntityData::PointCloudEx(data) = &extended.data else {
                return None;
            };
            let common = entity.common();
            let scans = point_cloud::resolve_source(document, data)
                .and_then(|path| point_cloud::load(&path))
                .map(|cloud| cloud.scan_ranges.iter().map(|(name, _)| name.clone()).collect())
                .unwrap_or_default();
            Some(Cloud {
                handle: common.handle,
                name: if data.name.is_empty() { format!("{:X}", common.handle.value()) } else { data.name.clone() },
                hidden: point_cloud::hidden(common),
                scans,
            })
        })
        .collect();
    if all.iter().any(|cloud| selected.contains(&cloud.handle)) {
        all.into_iter().filter(|cloud| selected.contains(&cloud.handle)).collect()
    } else {
        all
    }
}

/// Whether a row's switch is on. A parent is on while any child is.
pub fn is_on(hidden: &[String], scans: &[String], row: &Row) -> bool {
    let off = |name: &str| hidden.iter().any(|h| h == name);
    let any_scan = scans.is_empty() || scans.iter().any(|s| !off(s));
    match row {
        Row::Scan(name) => !off(name),
        Row::Scans => any_scan,
        Row::Unassigned => !off(UNASSIGNED_OFF),
        Row::Cloud => !off(CLOUD_OFF) && (!off(UNASSIGNED_OFF) || any_scan),
    }
}

/// The hidden list after flipping `row`. Switching the cloud back on shows
/// everything under it.
pub fn toggled(hidden: &[String], scans: &[String], row: &Row) -> Vec<String> {
    fn set(out: &mut Vec<String>, name: &str, off: bool) {
        out.retain(|h| h != name);
        if off {
            out.push(name.to_string());
        }
    }
    let on = is_on(hidden, scans, row);
    let mut out = hidden.to_vec();
    match row {
        Row::Cloud if on => set(&mut out, CLOUD_OFF, true),
        Row::Cloud => out.clear(),
        Row::Unassigned => set(&mut out, UNASSIGNED_OFF, on),
        Row::Scans => scans.iter().for_each(|s| set(&mut out, s, on)),
        Row::Scan(name) => set(&mut out, name, on),
    }
    out
}

/// Keys of the rows that fold: each cloud and its Scans.
pub fn folding_keys(clouds: &[Cloud]) -> Vec<String> {
    clouds.iter().flat_map(|c| [key(c.handle), format!("{}/scans", key(c.handle))]).collect()
}

fn key(handle: Handle) -> String {
    format!("{:X}", handle.value())
}

fn msg(m: PcManagerMsg) -> Message {
    Message::PcManager(m)
}

/// One tree row: indent, fold arrow, icon, label and switch (`None` for a
/// row that has none to give, drawn off and disabled).
fn tree_row<'a>(
    state: &PcManager,
    key: String,
    depth: u16,
    fold: Option<bool>,
    icon: Element<'a, Message>,
    label: String,
    switch: Option<(bool, Message)>,
) -> Element<'a, Message> {
    let arrow: Element<'a, Message> = match fold {
        Some(open) => button(if open {
            crate::ui::icons::themed_arrow_down(10.0)
        } else {
            crate::ui::icons::themed_arrow_right(10.0)
        })
        .on_press(msg(PcManagerMsg::Expand(key.clone())))
        .style(button::text)
        .padding(2)
        .into(),
        None => Space::new().width(14).into(),
    };
    let enabled = switch.is_some();
    let label = text(label).size(12).width(Fill);
    let label = if enabled { label } else { label.style(crate::ui::style::common::muted_style) };
    let switch: Element<'a, Message> = match switch {
        Some((on, message)) => checkbox(on).on_toggle(move |_| message.clone()).size(14).into(),
        None => checkbox(false).size(14).into(),
    };
    let selected = state.selected.as_deref() == Some(key.as_str());
    let cells = row![Space::new().width(Length::Fixed(f32::from(depth) * 16.0)), arrow, icon, label, switch]
        .spacing(5)
        .align_y(iced::Center);
    mouse_area(
        container(cells)
            .width(Fill)
            .padding([3, 6])
            .style(move |theme: &Theme| container::Style {
                background: selected.then(|| Background::Color(theme.palette().primary.weak.color)),
                text_color: selected.then(|| theme.palette().primary.weak.text),
                ..Default::default()
            }),
    )
    .on_press(msg(PcManagerMsg::Select(key)))
    .into()
}

fn fold_button<'a>(icon: &'static [u8], tip: String, message: PcManagerMsg) -> Element<'a, Message> {
    let b = button(crate::ui::icons::themed_secondary(icon, 12.0))
        .on_press(msg(message))
        .style(button::subtle)
        .padding([2, 4]);
    tooltip(b, text(tip).size(10), tooltip::Position::Bottom).gap(4).into()
}

pub fn view<'a>(
    state: &'a PcManager,
    document: &'a CadDocument,
    selected: &[Handle],
    width: f32,
    auto_collapse: bool,
) -> Element<'a, Message> {
    let title_bar =
        crate::ui::dock::title_bar(PanelId::PointCloudManager, crate::t!("Point Cloud Manager").into_owned(), auto_collapse);
    let query = state.search.trim().to_lowercase();
    let matches = |label: &str| query.is_empty() || label.to_lowercase().contains(&query);
    // Searching opens every node so the matches show.
    let open = |key: &str| !query.is_empty() || !state.collapsed.contains(key);
    let regions = crate::t!("Regions").into_owned();
    let unassigned = crate::t!("Unassigned Points").into_owned();
    let scans_label = crate::t!("Scans").into_owned();

    let mut tree = column![].spacing(1);
    for cloud in clouds(document, selected) {
        let root = key(cloud.handle);
        let scans: Vec<&String> = cloud.scans.iter().filter(|s| matches(s)).collect();
        let show_scans = matches(&scans_label) || !scans.is_empty();
        let children = matches(&regions) || matches(&unassigned) || show_scans;
        if !matches(&cloud.name) && !children {
            continue;
        }
        let switch = |row: Row| {
            let on = is_on(&cloud.hidden, &cloud.scans, &row);
            Some((on, msg(PcManagerMsg::Toggle(cloud.handle, row))))
        };
        let cloud_icon = || crate::ui::icons::semantic(CLOUD_ICON, 14.0);
        tree = tree.push(tree_row(
            state,
            root.clone(),
            0,
            Some(open(&root)),
            cloud_icon(),
            cloud.name.clone(),
            switch(Row::Cloud),
        ));
        if !open(&root) {
            continue;
        }
        if matches(&regions) {
            // Region data is not read: the row is there, off and disabled.
            tree = tree.push(tree_row(
                state,
                format!("{root}/regions"),
                1,
                None,
                crate::ui::icons::semantic_disabled(REGION_ICON, 14.0),
                regions.clone(),
                None,
            ));
        }
        if matches(&unassigned) {
            tree = tree.push(tree_row(
                state,
                format!("{root}/unassigned"),
                1,
                None,
                crate::ui::icons::semantic(UNASSIGNED_ICON, 14.0),
                unassigned.clone(),
                switch(Row::Unassigned),
            ));
        }
        if show_scans {
            let scans_key = format!("{root}/scans");
            let scans_open = open(&scans_key);
            tree = tree.push(tree_row(
                state,
                scans_key.clone(),
                1,
                Some(scans_open),
                cloud_icon(),
                scans_label.clone(),
                switch(Row::Scans),
            ));
            if scans_open {
                for name in scans {
                    tree = tree.push(tree_row(
                        state,
                        format!("{scans_key}/{name}"),
                        2,
                        None,
                        cloud_icon(),
                        name.clone(),
                        switch(Row::Scan(name.clone())),
                    ));
                }
            }
        }
    }

    let heading = text(crate::t!("REGIONS AND SCANS").into_owned())
        .size(11)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            style: iced::font::Style::Italic,
            ..iced::Font::DEFAULT
        })
        .style(|theme: &Theme| text::Style { color: Some(theme.palette().primary.base.color) })
        .width(Fill);
    let header = row![
        heading,
        fold_button(crate::ui::icons::MINUS, crate::t!("Collapse all").into_owned(), PcManagerMsg::CollapseAll),
        fold_button(crate::ui::icons::PLUS, crate::t!("Expand all").into_owned(), PcManagerMsg::ExpandAll),
    ]
    .spacing(2)
    .align_y(iced::Center);
    let card = container(column![header, scrollable(tree).height(Fill)].spacing(6))
        .padding(6)
        .width(Fill)
        .height(Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.palette().background.weak.color)),
            border: Border { radius: 4.0.into(), ..Default::default() },
            ..Default::default()
        });
    let search = text_input(&crate::t!("Search"), &state.search)
        .on_input(|v| msg(PcManagerMsg::Search(v)))
        .padding([4, 8])
        .size(12);

    crate::ui::dock::frame(column![title_bar, card, search].spacing(6), width)
}
