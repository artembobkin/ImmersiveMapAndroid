// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The engine a host drives: gestures in, frames out. It holds the camera
//! (`map_camera::controller`), resolves each frame's view in one call
//! (`map_frame::view`: presentation, render camera, tile coverage) and draws
//! the tiles the view names.
//!
//! What it draws so far is the coverage itself, each tile a square coloured
//! by its zoom: the tiles' contents come with the tile loader. The map is
//! the plane at every zoom until the globe's draw is ported.

use map_camera::controller::{CameraPositionError, CameraStateController};
use map_camera::flight::{CameraFlightAnimator, FlightAltitude, FlightRoute};
use map_camera::follow::{ZoomAnchorInput, zoom_anchor_center};
use map_camera::{CameraSettings, CameraState, PresentationSettings, RenderCamera};
use map_frame::view::{FrameView, ViewInput, ViewSettings};
use map_geo::matrix;
use map_visibility::rule_sets::RingRuleSets;

const SHADER: &str = include_str!("tile_grid.wgsl");
/// Floats per tile instance: the origin's x and y, the size, then the colour.
const INSTANCE_FLOATS: usize = 7;
/// The colour behind the map: the sky's, until the sky is drawn.
const CLEAR_COLOR: wgpu::Color = wgpu::Color { r: 0.62, g: 0.74, b: 0.86, a: 1.0 };

#[derive(Debug)]
pub enum EngineError {
    /// No GPU adapter draws to the surface.
    NoAdapter(String),
    NoDevice(String),
    Surface(String),
    Camera(CameraPositionError),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::NoAdapter(why) => write!(f, "no GPU adapter: {why}"),
            EngineError::NoDevice(why) => write!(f, "no GPU device: {why}"),
            EngineError::Surface(why) => write!(f, "the surface cannot be drawn to: {why}"),
            EngineError::Camera(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// What a frame drew.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// The tiles of the frame's coverage, each drawn as one instance.
    pub tiles: usize,
    /// The zoom the coverage targets.
    pub target_zoom: i32,
    /// Moves when the coverage changes.
    pub coverage_version: u32,
}

/// Where the frames go: a window's surface, or a texture for a host with no window.
enum Target {
    Surface { surface: wgpu::Surface<'static>, config: wgpu::SurfaceConfiguration },
    Texture(wgpu::Texture),
}

pub struct MapEngine {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,
    pipeline: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    instances: Vec<f32>,
    camera: CameraStateController,
    view: FrameView,
    settings: ViewSettings,
    /// The drawable's size in device pixels.
    width: u32,
    height: u32,
    /// Device pixels per point: gestures arrive in points.
    pixels_per_point: f64,
    /// The camera's animation, the double tap's flight.
    flight: CameraFlightAnimator,
}

/// How many times a drag across the view's full height sweeps the reachable
/// pitch, as `tiltGestureSensitivity` defaults to on the web. Negative
/// would invert the drag.
pub const TILT_GESTURE_SENSITIVITY: f64 = 2.0;

/// The pitch change of a vertical tilt drag of `translation_y` points in a
/// view `view_height` points high: down tilts further, up levels off.
pub fn tilt_drag_pitch_delta(translation_y: f64, view_height: f64, maximum_pitch: f64, sensitivity: f64) -> f64 {
    if view_height.is_nan() || view_height <= 0.0 {
        return 0.0;
    }
    translation_y / view_height * maximum_pitch.max(0.0) * sensitivity
}

/// The share of the fingers' turn the map turns by, as
/// `rotationGestureSensitivity` defaults to on the web: positive turns the
/// map with the fingers.
pub const ROTATION_GESTURE_SENSITIVITY: f64 = 0.6;

/// The double tap's anchored zoom: one level in, over this many seconds, as on the web.
pub const DOUBLE_TAP_ZOOM_DELTA: f64 = 1.0;
pub const DOUBLE_TAP_FLIGHT_DURATION: f64 = 0.35;

fn view_settings() -> ViewSettings {
    ViewSettings { presentation: PresentationSettings { is_globe_enabled: false, ..PresentationSettings::default() }, maximum_zoom_level: 15, raster_band_margin_share: 0.0, rule_sets: RingRuleSets::default() }
}

/// The colour a tile of a zoom is drawn in: a hue a zoom, so the bands of the coverage read apart.
fn zoom_color(zoom: i32) -> [f32; 4] {
    let hue = (zoom as f32 * 0.137).fract() * 6.0;
    let channel = |offset: f32| {
        let k = (offset + hue) % 6.0;
        0.92 - 0.3 * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [channel(5.0), channel(3.0), channel(1.0), 1.0]
}

impl MapEngine {
    /// An engine that draws to a surface of `width` by `height` device pixels.
    pub async fn with_surface(instance: &wgpu::Instance, surface: wgpu::Surface<'static>, width: u32, height: u32, pixels_per_point: f64) -> Result<MapEngine, EngineError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), force_fallback_adapter: false, apply_limit_buckets: false })
            .await
            .map_err(|error| EngineError::NoAdapter(error.to_string()))?;
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.map_err(|error| EngineError::NoDevice(error.to_string()))?;
        let mut config = surface.get_default_config(&adapter, width.max(1), height.max(1)).ok_or_else(|| EngineError::Surface("the adapter has no format for it".to_owned()))?;
        // Frames are drawn on demand and shown at the display's pace.
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);
        let format = config.format;
        Ok(MapEngine::new(device, queue, Target::Surface { surface, config }, format, width, height, pixels_per_point))
    }

    /// An engine that draws to a texture: for a host with no window, the tests among them.
    pub async fn offscreen(width: u32, height: u32) -> Result<MapEngine, EngineError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })
            .await
            .map_err(|error| EngineError::NoAdapter(error.to_string()))?;
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.map_err(|error| EngineError::NoDevice(error.to_string()))?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Ok(MapEngine::new(device, queue, Target::Texture(texture), format, width, height, 1.0))
    }

    fn new(device: wgpu::Device, queue: wgpu::Queue, target: Target, format: wgpu::TextureFormat, width: u32, height: u32, pixels_per_point: f64) -> MapEngine {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("tile grid"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
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
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fragment"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
            multiview_mask: None,
            cache: None,
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("frame"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("frame"), layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() }] });
        let instance_capacity = 256;
        let instance_buffer = Self::make_instance_buffer(&device, instance_capacity);
        let mut camera = CameraStateController::new(CameraSettings::default());
        // Moscow's centre, as the engine's other ports start.
        camera.set_camera_position(55.751244, 37.618423, 12.0, 0.0, 0.0).expect("a position within the map");
        MapEngine { device, queue, target, pipeline, frame_buffer, bind_group, instance_buffer, instance_capacity, instances: Vec::new(), camera, view: FrameView::new(), settings: view_settings(), width: width.max(1), height: height.max(1), pixels_per_point: pixels_per_point.max(0.1), flight: CameraFlightAnimator::default() }
    }

    fn make_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some("tiles"), size: (capacity * INSTANCE_FLOATS * 4) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }

    /// The drawable changed its size, in device pixels.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        if let Target::Surface { surface, config } = &mut self.target {
            config.width = self.width;
            config.height = self.height;
            surface.configure(&self.device, config);
        }
    }

    pub fn camera_state(&self) -> CameraState {
        self.camera.camera_state()
    }

    /// The camera at a latitude and longitude in degrees, the bearing and the pitch in radians.
    pub fn set_camera_position(&mut self, latitude: f64, longitude: f64, zoom: f64, bearing: f64, pitch: f64) -> Result<(), EngineError> {
        self.flight.cancel();
        self.camera.set_camera_position(latitude, longitude, zoom, bearing, pitch).map_err(EngineError::Camera)
    }

    /// A drag by a distance in points.
    pub fn pan(&mut self, delta_x: f64, delta_y: f64) {
        let scale = CameraSettings::default().gesture_pan_translation_scale;
        self.camera.pan(delta_x * scale, delta_y * scale, self.transition(), None);
    }

    /// The drag ended.
    pub fn end_pan(&mut self) {
        self.camera.end_pan();
    }

    /// A pinch by `scale` about a point of the view, in points from its top
    /// left: the map under the point stays under it.
    pub fn zoom_about(&mut self, scale: f64, focus_x: f64, focus_y: f64) {
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let before = self.camera.camera_state();
        let transition = self.transition();
        self.camera.zoom_by_delta(scale.log2(), None);
        let after = self.camera.camera_state();
        let center = zoom_anchor_center(&ZoomAnchorInput {
            anchor_point: [focus_x, focus_y],
            viewport_size: [self.width as f64 / self.pixels_per_point, self.height as f64 / self.pixels_per_point],
            center_world_mercator: after.center_world_mercator,
            zoom_before: before.zoom,
            zoom_after: after.zoom,
            bearing: after.bearing,
            transition_before: transition,
            transition_after: self.transition(),
            globe_radius_scale: self.settings.presentation.globe_radius_scale,
            anchor_factor: 1.0,
        });
        self.camera.set_center_world_mercator(center);
    }

    /// A turn of the map by an angle, radians.
    pub fn rotate(&mut self, delta: f64) {
        self.camera.rotate_yaw(delta);
    }

    /// A two-finger turn by the fingers' angle, radians, clockwise on screen
    /// positive: the map turns with them by [`ROTATION_GESTURE_SENSITIVITY`].
    pub fn rotate_by_gesture(&mut self, angle: f64) {
        self.rotate(angle * ROTATION_GESTURE_SENSITIVITY);
    }

    /// A double tap at a point of the view, in points from its top left:
    /// the camera flies [`DOUBLE_TAP_ZOOM_DELTA`] levels in toward it over
    /// [`DOUBLE_TAP_FLIGHT_DURATION`] seconds from `now`, in seconds, and
    /// lands with the map under the point still under it. False when there
    /// is nowhere to fly, at the deepest zoom.
    pub fn double_tap_zoom(&mut self, x: f64, y: f64, now: f64) -> bool {
        let start = self.camera.camera_state();
        self.zoom_about(DOUBLE_TAP_ZOOM_DELTA.exp2(), x, y);
        let target = self.camera.camera_state();
        self.camera.set_camera_state(start);
        self.flight.start(start, target, DOUBLE_TAP_FLIGHT_DURATION, FlightRoute::MercatorShortestPath, FlightAltitude::Direct, now)
    }

    /// Moves the camera's animation to `now`, in seconds on the clock its
    /// start was given. True while it goes on: the host draws another frame.
    pub fn advance_animations(&mut self, now: f64) -> bool {
        match self.flight.advance(now) {
            Some(step) => {
                self.camera.set_camera_state(step.camera_state);
                !step.did_finish
            }
            None => false,
        }
    }

    /// Stops the camera's animation where it is: a gesture takes the camera.
    pub fn cancel_animations(&mut self) {
        self.flight.cancel();
    }

    /// A tilt of the camera by an angle, radians, within the settings' range.
    pub fn tilt(&mut self, delta: f64) {
        let pitch = self.camera.camera_state().pitch;
        self.camera.set_pitch(pitch + delta);
    }

    /// A two-finger tilt drag by a vertical distance in points, down
    /// tilting further: the view's full height sweeps the reachable pitch
    /// [`TILT_GESTURE_SENSITIVITY`] times over, as on the other ports.
    pub fn tilt_by_drag(&mut self, translation_y: f64) {
        let state = self.camera.camera_state();
        let maximum_pitch = CameraSettings::default().maximum_reachable_pitch(state.zoom);
        let view_height = self.height as f64 / self.pixels_per_point;
        self.tilt(tilt_drag_pitch_delta(translation_y, view_height, maximum_pitch, TILT_GESTURE_SENSITIVITY));
    }

    fn transition(&self) -> f64 {
        map_camera::Presentation::resolve(&self.camera.camera_state(), &self.settings.presentation, None).transition
    }

    /// Draws a frame and shows it.
    pub fn render(&mut self) -> Result<FrameStats, EngineError> {
        let input = ViewInput { camera_state: self.camera.camera_state(), planning_zoom: self.camera.planning_zoom(), draw_width: self.width as f64, draw_height: self.height as f64, forced_surface: None, relief_heights: None };
        let Some(view) = self.view.resolve(&self.settings, &input) else {
            return Ok(FrameStats { tiles: 0, target_zoom: 0, coverage_version: 0 });
        };
        // The GPU's far plane is far past the CPU's, which the coverage is cut by.
        let projection_view = matrix::multiply(&RenderCamera::gpu_clip_depth_adjustment(), &view.camera.projection_view);
        let mut uniform = [0u8; 64];
        for (bytes, value) in uniform.as_chunks_mut::<4>().0.iter_mut().zip(projection_view) {
            bytes.copy_from_slice(&(value as f32).to_le_bytes());
        }
        self.queue.write_buffer(&self.frame_buffer, 0, &uniform);
        // The targets come finest first: drawn in reverse, a finer tile covers the coarser one under it.
        self.instances.clear();
        for target in view.content.targets.iter().rev() {
            let tile = target.tile;
            let origin = view.presentation.flat.tile_origin_and_size(tile.x, tile.y, tile.z, tile.wrap);
            self.instances.extend_from_slice(&[origin[0] as f32, origin[1] as f32, origin[2] as f32]);
            self.instances.extend_from_slice(&zoom_color(tile.z));
        }
        let stats = FrameStats { tiles: view.content.targets.len(), target_zoom: view.target_zoom, coverage_version: view.content.coverage_version };
        if stats.tiles > self.instance_capacity {
            self.instance_capacity = stats.tiles.next_power_of_two();
            self.instance_buffer = Self::make_instance_buffer(&self.device, self.instance_capacity);
        }
        let mut bytes = Vec::with_capacity(self.instances.len() * 4);
        for value in &self.instances {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.queue.write_buffer(&self.instance_buffer, 0, &bytes);

        let (frame, texture_view) = match &self.target {
            Target::Surface { surface, config } => match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                    let texture_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    (Some(frame), texture_view)
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    // The surface changed under the frame: it is configured again and the next frame draws.
                    surface.configure(&self.device, config);
                    return Ok(stats);
                }
                other => return Err(EngineError::Surface(format!("{other:?}"))),
            },
            Target::Texture(texture) => (None, texture.create_view(&wgpu::TextureViewDescriptor::default())),
        };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &texture_view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(CLEAR_COLOR), store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            if stats.tiles > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instance_buffer.slice(..(bytes.len() as u64)));
                pass.draw(0..4, 0..stats.tiles as u32);
            }
        }
        self.queue.submit([encoder.finish()]);
        if let Some(frame) = frame {
            self.queue.present(frame);
        }
        Ok(stats)
    }

    /// The device, for a host that reads a frame back or watches for errors.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
}
