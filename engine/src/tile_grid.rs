// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The coverage drawn as it is, each tile a square coloured by its zoom,
//! in the overlay's tile grid layer, where the web draws its tile grid: the
//! bands the map is drawn in, until the tiles' contents are.

use crate::frame::{FrameContext, FrameFormats, RenderSubsystem};
use crate::render_layer::{RenderLayer, RenderPassAvailability};

const SHADER: &str = include_str!("tile_grid.wgsl");
/// Floats per tile instance: the origin's x and y, the size, then the colour.
const INSTANCE_FLOATS: usize = 7;

/// The colour a tile of a zoom is drawn in: a hue a zoom, so the bands of the coverage read apart.
fn zoom_color(zoom: i32) -> [f32; 4] {
    let hue = (zoom as f32 * 0.137).fract() * 6.0;
    let channel = |offset: f32| {
        let k = (offset + hue) % 6.0;
        0.92 - 0.3 * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [channel(5.0), channel(3.0), channel(1.0), 1.0]
}

pub struct TileGridSubsystem {
    device: wgpu::Device,
    pipeline: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    instances: Vec<f32>,
    /// The tiles of the last frame's coverage.
    tiles: usize,
}

impl TileGridSubsystem {
    pub fn new(device: &wgpu::Device, formats: FrameFormats) -> TileGridSubsystem {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("tile grid"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile grid frame"),
            entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(64) }, count: None }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("tile grid"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tile grid"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: (INSTANCE_FLOATS * 4) as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }, wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 12, shader_location: 1 }],
                })],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            // The overlay's depth, neither tested nor written: the squares are drawn in order, finer over coarser.
            depth_stencil: Some(wgpu::DepthStencilState { format: formats.overlay_depth, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fragment"), compilation_options: Default::default(), targets: &[Some(formats.color.into())] }),
            multiview_mask: None,
            cache: None,
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("tile grid frame"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("tile grid frame"), layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() }] });
        let instance_capacity = 256;
        TileGridSubsystem { device: device.clone(), pipeline, frame_buffer, bind_group, instance_buffer: Self::make_instance_buffer(device, instance_capacity), instance_capacity, instances: Vec::new(), tiles: 0 }
    }

    fn make_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("tile grid tiles"), size: (capacity * INSTANCE_FLOATS * 4) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }

}

impl RenderSubsystem for TileGridSubsystem {
    fn name(&self) -> &'static str {
        "tile grid"
    }

    fn layers(&self) -> &'static [RenderLayer] {
        &[RenderLayer::TileGrid]
    }

    fn update(&mut self, context: &FrameContext, queue: &wgpu::Queue) {
        let view = context.view;
        // The targets come finest first: drawn in reverse, a finer tile covers the coarser one under it.
        self.instances.clear();
        for target in view.content.targets.iter().rev() {
            let tile = target.tile;
            let origin = view.presentation.flat.tile_origin_and_size(tile.x, tile.y, tile.z, tile.wrap);
            self.instances.extend_from_slice(&[origin[0] as f32, origin[1] as f32, origin[2] as f32]);
            self.instances.extend_from_slice(&zoom_color(tile.z));
        }
        self.tiles = view.content.targets.len();
        if self.tiles == 0 {
            return;
        }
        if self.tiles > self.instance_capacity {
            self.instance_capacity = self.tiles.next_power_of_two();
            self.instance_buffer = Self::make_instance_buffer(&self.device, self.instance_capacity);
        }
        let mut uniform = [0u8; 64];
        for (bytes, value) in uniform.as_chunks_mut::<4>().0.iter_mut().zip(context.projection_view) {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.frame_buffer, 0, &uniform);
        let bytes: Vec<u8> = self.instances.iter().flat_map(|value| value.to_le_bytes()).collect();
        queue.write_buffer(&self.instance_buffer, 0, &bytes);
    }

    fn contribute_availability(&self, availability: &mut RenderPassAvailability) {
        availability.tile_grid_enabled = self.tiles > 0;
    }

    fn encode(&self, _layer: RenderLayer, pass: &mut wgpu::RenderPass<'_>, _context: &FrameContext) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..(self.tiles * INSTANCE_FLOATS * 4) as u64));
        pass.draw(0..4, 0..self.tiles as u32);
    }
}
