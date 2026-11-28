// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The engine a host drives: gestures in, frames out. It holds the camera
//! (`map_camera::controller`), resolves each frame's view in one call
//! (`map_frame::view`: presentation, render camera, tile coverage) and draws
//! the tiles the view names.
//!
//! The frame is drawn by `frame::FrameEngine`, which runs the subsystems'
//! layers in their passes. What it draws so far is the coverage itself,
//! each tile a square coloured by its zoom (`tile_grid`): the tiles'
//! contents come with the tile loader. The map is the plane at every zoom
//! until the globe's draw is ported.

use map_camera::controller::{CameraPositionError, CameraStateController};
use map_camera::flight::{CameraFlightAnimator, FlightAltitude, FlightRoute};
use map_camera::follow::{ZoomAnchorInput, zoom_anchor_center};
use map_camera::{CameraSettings, CameraState, PresentationSettings, RenderCamera};
use map_frame::view::{FrameView, ViewInput, ViewSettings};
use map_geo::matrix;
use map_visibility::rule_sets::RingRuleSets;

use crate::frame::{FrameContext, FrameEngine, FrameFormats, FrameSetup, RenderSubsystem};
use crate::gpu::GpuContext;
use crate::render_layer::{RenderPassAvailability, RenderSurfaceMode};
use crate::tile_grid::TileGridSubsystem;
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
    gpu: GpuContext,
    target: Target,
    frame_engine: FrameEngine,
    tile_grid: TileGridSubsystem,
    frame_number: u64,
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

/// What a frame draws before the subsystems add theirs: the plane, with
/// nothing of the layers whose content is not ported yet.
fn availability() -> RenderPassAvailability {
    RenderPassAvailability { render_surface_mode: RenderSurfaceMode::Flat, labels_enabled: false, pins_enabled: false, debug_overlay_enabled: false, starfield_enabled: false, scene_models_enabled: false, app_layers_enabled: false, tile_grid_enabled: false }
}

fn view_settings() -> ViewSettings {
    ViewSettings { presentation: PresentationSettings { is_globe_enabled: false, ..PresentationSettings::default() }, maximum_zoom_level: 15, raster_band_margin_share: 0.0, rule_sets: RingRuleSets::default() }
}

impl MapEngine {
    /// An engine that draws to a surface of `width` by `height` device pixels.
    pub async fn with_surface(instance: &wgpu::Instance, surface: wgpu::Surface<'static>, width: u32, height: u32, pixels_per_point: f64) -> Result<MapEngine, EngineError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), force_fallback_adapter: false, apply_limit_buckets: false })
            .await
            .map_err(|error| EngineError::NoAdapter(error.to_string()))?;
        let gpu = GpuContext::new(&adapter).await.map_err(|error| EngineError::NoDevice(error.to_string()))?;
        let mut config = surface.get_default_config(&adapter, width.max(1), height.max(1)).ok_or_else(|| EngineError::Surface("the adapter has no format for it".to_owned()))?;
        // Frames are drawn on demand and shown at the display's pace.
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&gpu.device, &config);
        let format = config.format;
        Ok(MapEngine::new(gpu, Target::Surface { surface, config }, format, width, height, pixels_per_point))
    }

    /// An engine that draws to a texture: for a host with no window, the tests among them.
    pub async fn offscreen(width: u32, height: u32) -> Result<MapEngine, EngineError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })
            .await
            .map_err(|error| EngineError::NoAdapter(error.to_string()))?;
        let gpu = GpuContext::new(&adapter).await.map_err(|error| EngineError::NoDevice(error.to_string()))?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Ok(MapEngine::new(gpu, Target::Texture(texture), format, width, height, 1.0))
    }

    fn new(gpu: GpuContext, target: Target, format: wgpu::TextureFormat, width: u32, height: u32, pixels_per_point: f64) -> MapEngine {
        let formats = FrameFormats { color: format, world_depth_stencil: gpu.capabilities.depth_stencil_format(), overlay_depth: wgpu::TextureFormat::Depth32Float };
        let tile_grid = TileGridSubsystem::new(&gpu.device, formats);
        let mut camera = CameraStateController::new(CameraSettings::default());
        // Moscow's centre, as the engine's other ports start.
        camera.set_camera_position(55.751244, 37.618423, 12.0, 0.0, 0.0).expect("a position within the map");
        MapEngine { gpu, target, frame_engine: FrameEngine::new(formats), tile_grid, frame_number: 0, camera, view: FrameView::new(), settings: view_settings(), width: width.max(1), height: height.max(1), pixels_per_point: pixels_per_point.max(0.1), flight: CameraFlightAnimator::default() }
    }

    /// The drawable changed its size, in device pixels.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        if let Target::Surface { surface, config } = &mut self.target {
            config.width = self.width;
            config.height = self.height;
            surface.configure(&self.gpu.device, config);
        }
        if let Target::Texture(texture) = &mut self.target {
            // The texture is made again at the new size, as a surface is configured again.
            *texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor { label: Some("frame"), size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: texture.format(), usage: texture.usage(), view_formats: &[] });
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
        let stats = FrameStats { tiles: view.content.targets.len(), target_zoom: view.target_zoom, coverage_version: view.content.coverage_version };
        let (frame, texture_view) = match &self.target {
            Target::Surface { surface, config } => match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                    let texture_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    (Some(frame), texture_view)
                }
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    // The surface changed under the frame: it is configured again and the next frame draws.
                    surface.configure(&self.gpu.device, config);
                    return Ok(stats);
                }
                other => return Err(EngineError::Surface(format!("{other:?}"))),
            },
            Target::Texture(texture) => (None, texture.create_view(&wgpu::TextureViewDescriptor::default())),
        };
        // The GPU's far plane is far past the CPU's, which the coverage is cut by.
        let projection_view = matrix::multiply(&RenderCamera::gpu_clip_depth_adjustment(), &view.camera.projection_view).map(|value| value as f32);
        self.frame_number += 1;
        let context = FrameContext { frame_number: self.frame_number, draw_width: self.width, draw_height: self.height, pixels_per_point: self.pixels_per_point, planning_zoom: self.camera.planning_zoom(), view: &view, projection_view };
        let setup = FrameSetup { availability: availability(), clear_color: CLEAR_COLOR };
        let mut subsystems: [&mut dyn RenderSubsystem; 1] = [&mut self.tile_grid];
        self.frame_engine.frame(&self.gpu.device, &self.gpu.queue, &texture_view, &setup, &context, &mut subsystems);
        if let Some(frame) = frame {
            self.gpu.queue.present(frame);
        }
        Ok(stats)
    }

    /// The GPU the engine draws with: the adapter, what the device took of it, the limits.
    pub fn gpu_report(&self) -> String {
        self.gpu.report()
    }

    /// What the device can do beyond core WebGPU.
    pub fn gpu_capabilities(&self) -> crate::gpu::GpuCapabilities {
        self.gpu.capabilities
    }

    /// The device, for a host that reads a frame back or watches for errors.
    pub fn device(&self) -> &wgpu::Device {
        &self.gpu.device
    }
}
