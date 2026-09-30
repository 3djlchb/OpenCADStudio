//! PCEXTRACTEDGE, PCEXTRACTCORNER, PCEXTRACTCENTERLINE and
//! (-)PCEXTRACTSECTION: the commands get the drawing's shown cloud points
//! and its section objects when they start.

use super::*;
use codec::entities::ExtendedEntityData;
use glam::DVec3;

use crate::modules::insert::pc_extract::{
    CenterlineCommand, Clouds, PlanesCommand, Section, SectionCommand,
};

fn dvec(v: codec::types::Vector3) -> DVec3 {
    DVec3::new(v.x, v.y, v.z)
}

impl OpenCADStudio {
    /// The points each point cloud shows (the renderer's placed set).
    fn extraction_clouds(&self, i: usize) -> Clouds {
        let document = &self.tabs[i].scene.document;
        let mut clouds = Vec::new();
        for entity in document.entities() {
            let codec::EntityType::Extended(extended) = entity else {
                continue;
            };
            let ExtendedEntityData::PointCloudEx(data) = &extended.data else {
                continue;
            };
            let common = entity.common();
            // The colour the renderer places with, so its cached points are reused.
            let color = if matches!(common.color, codec::types::Color::ByLayer) {
                document.layers.get(&common.layer).map(|layer| layer.color.clone()).unwrap_or(common.color.clone())
            } else {
                common.color.clone()
            };
            let color = color.rgb().map_or([255; 3], |(r, g, b)| [r, g, b]);
            if let Some(placed) = crate::scene::model::point_cloud::placed(
                document,
                data,
                color,
                None,
                &crate::scene::model::point_cloud::hidden(data),
            ) {
                clouds.push((common.handle, placed));
            }
        }
        Clouds(clouds)
    }

    fn extraction_sections(&self, i: usize) -> Vec<Section> {
        self.tabs[i]
            .scene
            .document
            .entities()
            .filter_map(|entity| {
                let codec::EntityType::Extended(extended) = entity else {
                    return None;
                };
                let ExtendedEntityData::SectionObject(data) = &extended.data else {
                    return None;
                };
                let (first, last) = (dvec(*data.vertices.first()?), dvec(*data.vertices.last()?));
                let tangent = (last - first).normalize_or(DVec3::X);
                let vertical = dvec(data.vertical_direction).normalize_or(DVec3::Z);
                let viewing = vertical.cross(tangent).normalize_or(-DVec3::Z)
                    * if data.flags & 4 != 0 { 1.0 } else { -1.0 };
                Some(Section { origin: first, viewing, tangent, live: data.flags & 1 != 0 })
            })
            .collect()
    }

    pub(super) fn dispatch_pc_extract(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        use crate::command::CadCommand;
        let command: Box<dyn CadCommand> = match cmd {
            "PCEXTRACTEDGE" => Box::new(PlanesCommand::new(false, self.extraction_clouds(i))),
            "PCEXTRACTCORNER" => Box::new(PlanesCommand::new(true, self.extraction_clouds(i))),
            "PCEXTRACTCENTERLINE" => Box::new(CenterlineCommand::new(self.extraction_clouds(i))),
            "PCEXTRACTSECTION" | "-PCEXTRACTSECTION" => Box::new(SectionCommand::new(
                cmd == "PCEXTRACTSECTION",
                self.extraction_clouds(i),
                self.extraction_sections(i),
            )),
            _ => return None,
        };
        self.command_line.push_info(&command.prompt());
        self.tabs[i].active_cmd = Some(command);
        Some(self.finish_dispatch(cmd))
    }
}
