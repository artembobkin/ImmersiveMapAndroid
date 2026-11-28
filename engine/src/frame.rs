// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The map's frame, shaped as the web's `render/frame-engine.ts` (the Swift
//! `RenderFrameEngine`): each subsystem updates, says what it has to draw,
//! encodes its GPU work, then draws the layers it owns into the pass that
//! hosts them, in the plan's order (`render_layer`).
//!
//! The passes, in the web's order: the shadow map's casters, the ground's
//! shadow mask, the world, the scene depth's resolve, the post-processing,
//! the overlay. This port has the world and the overlay so far; the others
//! come with the subsystems that draw into them. The world pass draws over
//! a depth and stencil that are cleared and never kept, the overlay over a
//! depth of its own (the labels' halo ordering).

use map_frame::view::View;

use crate::render_layer::{RenderLayer, RenderPassAvailability, pass_layers};

/// What every subsystem reads of a frame.
pub struct FrameContext<'a> {
    pub frame_number: u64,
    /// The drawable's size, in device pixels.
    pub draw_width: u32,
    pub draw_height: u32,
    pub pixels_per_point: f64,
    /// The camera's planning zoom, the one tiles and style fades answer.
    pub planning_zoom: f64,
    /// The frame's view of the shared core: presentation, render camera, coverage.
    pub view: &'a View<'a>,
    /// The projection-view the GPU draws with, its depth in the GPU's range.
    pub projection_view: [f32; 16],
}

/// A feature's drawing, the web's `RenderSubsystem`.
pub trait RenderSubsystem {
    fn name(&self) -> &'static str;
    /// The layers it draws.
    fn layers(&self) -> &'static [RenderLayer];
    /// The frame's CPU work and uploads, before anything is encoded.
    fn update(&mut self, _context: &FrameContext, _queue: &wgpu::Queue) {}
    /// After `update`: what the subsystem's content turns on in the frame's
    /// passes, such as the tile grid's layer when there are tiles.
    fn contribute_availability(&self, _availability: &mut RenderPassAvailability) {}
    /// The GPU work the passes read, compute passes among it, encoded before them.
    fn prepare_gpu(&mut self, _context: &FrameContext, _encoder: &mut wgpu::CommandEncoder) {}
    /// Draws one of its layers into the pass that hosts it.
    fn encode(&self, layer: RenderLayer, pass: &mut wgpu::RenderPass<'_>, context: &FrameContext);
    /// After the frame's submit.
    fn frame_committed(&mut self) {}
}

/// What a frame is drawn with beyond its subsystems.
#[derive(Debug, Clone, Copy)]
pub struct FrameSetup {
    /// What the frame draws before the subsystems add theirs.
    pub availability: RenderPassAvailability,
    pub clear_color: wgpu::Color,
}

/// The formats the passes draw in, which every subsystem's pipelines are built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameFormats {
    pub color: wgpu::TextureFormat,
    pub world_depth_stencil: wgpu::TextureFormat,
    pub overlay_depth: wgpu::TextureFormat,
}

struct Attachments {
    width: u32,
    height: u32,
    world_depth_stencil: wgpu::TextureView,
    overlay_depth: wgpu::TextureView,
}

/// The passes of a frame, and what they planned.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FramePlan {
    pub world: Vec<RenderLayer>,
    pub overlay: Vec<RenderLayer>,
}

pub struct FrameEngine {
    formats: FrameFormats,
    attachments: Option<Attachments>,
}

impl FrameEngine {
    pub fn new(formats: FrameFormats) -> FrameEngine {
        FrameEngine { formats, attachments: None }
    }

    pub fn formats(&self) -> FrameFormats {
        self.formats
    }

    fn ensure_attachments(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &Attachments {
        let fits = self.attachments.as_ref().is_some_and(|attachments| attachments.width == width && attachments.height == height);
        if !fits {
            let make = |label, format| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            self.attachments = Some(Attachments { width, height, world_depth_stencil: make("world depth-stencil", self.formats.world_depth_stencil), overlay_depth: make("overlay depth", self.formats.overlay_depth) });
        }
        self.attachments.as_ref().expect("made above")
    }

    /// One frame into `target`, a view of `context`'s size.
    pub fn frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, setup: &FrameSetup, context: &FrameContext, subsystems: &mut [&mut dyn RenderSubsystem]) -> FramePlan {
        for subsystem in subsystems.iter_mut() {
            subsystem.update(context, queue);
        }
        let mut availability = setup.availability;
        for subsystem in subsystems.iter() {
            subsystem.contribute_availability(&mut availability);
        }
        let (world, overlay) = pass_layers(&availability);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        for subsystem in subsystems.iter_mut() {
            subsystem.prepare_gpu(context, &mut encoder);
        }
        let attachments = self.ensure_attachments(device, context.draw_width, context.draw_height);
        let encode_layer = |layer: RenderLayer, pass: &mut wgpu::RenderPass<'_>| {
            for subsystem in subsystems.iter().filter(|subsystem| subsystem.layers().contains(&layer)) {
                subsystem.encode(layer, pass, context);
            }
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(setup.clear_color), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &attachments.world_depth_stencil,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    // The stencil carries the tile-priority marks and the surface bits: cleared to 0, never kept.
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                }),
                ..Default::default()
            });
            for &layer in &world {
                encode_layer(layer, &mut pass);
            }
        }
        let has_overlay = overlay.iter().any(|layer| subsystems.iter().any(|subsystem| subsystem.layers().contains(layer)));
        if has_overlay {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &attachments.overlay_depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                ..Default::default()
            });
            for &layer in &overlay {
                encode_layer(layer, &mut pass);
            }
        }
        queue.submit([encoder.finish()]);
        for subsystem in subsystems.iter_mut() {
            subsystem.frame_committed();
        }
        FramePlan { world, overlay }
    }
}
