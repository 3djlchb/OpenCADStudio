// Point cloud GPU buffers — one instance buffer per placed cloud, drawn as
// screen-space square sprites (6 vertices per instance, no vertex buffer).
//
// Group 1 binding 0 — PointParams (sprite size in pixels, 16 bytes).
//
// Buffers are keyed by `PlacedCloud::key`: a new render set re-uploads only
// the clouds whose points changed; the rest keep their buffers.

use crate::scene::model::point_cloud::{PointCloudSet, PointInstance};
use iced::wgpu;

pub fn instance_layout<'a>() -> wgpu::VertexBufferLayout<'a> {
    const ATTRS: &[wgpu::VertexAttribute] = &[
        wgpu::VertexAttribute {
            offset: std::mem::offset_of!(PointInstance, pos) as u64,
            shader_location: 0,
            format: wgpu::VertexFormat::Float32x3,
        },
        wgpu::VertexAttribute {
            offset: std::mem::offset_of!(PointInstance, pos_low) as u64,
            shader_location: 1,
            format: wgpu::VertexFormat::Float32x3,
        },
        wgpu::VertexAttribute {
            offset: std::mem::offset_of!(PointInstance, color) as u64,
            shader_location: 2,
            format: wgpu::VertexFormat::Unorm8x4,
        },
    ];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<PointInstance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: ATTRS,
    }
}

pub fn params_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("point_cloud.bgl1"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

struct CloudBuffer {
    key: u64,
    buffer: wgpu::Buffer,
    count: u32,
}

/// The resident point clouds of one viewport pipeline.
pub struct PointCloudGpu {
    clouds: Vec<CloudBuffer>,
    params: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    point_size: f32,
}

impl PointCloudGpu {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point_cloud.params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("point_cloud.bind_group1"),
            layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }],
        });
        Self { clouds: Vec::new(), params, bind_group, point_size: f32::NAN }
    }

    pub fn is_empty(&self) -> bool {
        self.clouds.is_empty()
    }

    /// Make `set` resident, keeping the buffers of clouds it already had.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, set: &PointCloudSet) {
        let mut old = std::mem::take(&mut self.clouds);
        for cloud in set.clouds.iter().filter(|cloud| !cloud.instances.is_empty()) {
            if let Some(at) = old.iter().position(|resident| resident.key == cloud.key) {
                self.clouds.push(old.swap_remove(at));
                continue;
            }
            self.clouds.push(CloudBuffer {
                key: cloud.key,
                buffer: super::gpu_upload::upload_buffer(
                    device,
                    queue,
                    "point_cloud.instances",
                    &cloud.instances,
                    wgpu::BufferUsages::VERTEX,
                ),
                count: cloud.instances.len() as u32,
            });
        }
        if self.point_size != set.point_size {
            self.point_size = set.point_size;
            queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&[set.point_size, 0.0, 0.0, 0.0]));
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_bind_group(1, &self.bind_group, &[]);
        for cloud in &self.clouds {
            pass.set_vertex_buffer(0, cloud.buffer.slice(..));
            pass.draw(0..6, 0..cloud.count);
        }
    }
}
