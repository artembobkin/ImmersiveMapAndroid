// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The binding the Kotlin library calls through JNI
//! (`io.github.artembobkin.immersivemap.NativeEngine`). An engine is made
//! over a `Surface` and handed back as a handle. Every call for one engine
//! comes from one thread, the view's render thread, so nothing here locks.
//! A call that fails logs why and leaves the engine as it was: nothing
//! crosses back to Kotlin as an exception.

use std::ptr::NonNull;

use jni::JNIEnv;
use jni::objects::JObject;
use jni::sys::{jdouble, jfloat, jint, jlong};
use ndk::native_window::NativeWindow;
use raw_window_handle::{AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle};

use crate::engine::MapEngine;

/// The engine with the window it draws to, which must outlive the surface made of it.
struct AndroidEngine {
    engine: MapEngine,
    _window: NativeWindow,
}

fn init_logging() {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("ImmersiveMap"));
}

fn make(env: &JNIEnv, surface: &JObject, width: u32, height: u32, pixels_per_point: f64) -> Result<AndroidEngine, String> {
    // Safety: `surface` is a live `android.view.Surface` for the length of this call, and the window takes its own reference.
    let window = unsafe { NativeWindow::from_surface(env.get_raw().cast(), surface.as_raw().cast()) }.ok_or("the Surface has no native window")?;
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(descriptor);
    let window_handle = AndroidNdkWindowHandle::new(NonNull::new(window.ptr().as_ptr().cast()).ok_or("the native window is null")?);
    let target = wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(RawDisplayHandle::Android(AndroidDisplayHandle::new())), raw_window_handle: RawWindowHandle::AndroidNdk(window_handle) };
    // Safety: the window is kept beside the engine and dropped after it, so it outlives the surface.
    let surface = unsafe { instance.create_surface_unsafe(target) }.map_err(|error| error.to_string())?;
    let engine = pollster::block_on(MapEngine::with_surface(&instance, surface, width, height, pixels_per_point)).map_err(|error| error.to_string())?;
    Ok(AndroidEngine { engine, _window: window })
}

/// The engine behind a handle, `None` for the handle of none.
fn engine<'a>(handle: jlong) -> Option<&'a mut MapEngine> {
    // Safety: a handle is a pointer `create` leaked and `destroy` has not taken back yet, used from one thread.
    unsafe { (handle as *mut AndroidEngine).as_mut() }.map(|android| &mut android.engine)
}

/// An engine over a `Surface` of `width` by `height` pixels. Returns its handle, 0 when it could not be made.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_create(env: JNIEnv, _this: JObject, surface: JObject, width: jint, height: jint, density: jfloat) -> jlong {
    init_logging();
    match make(&env, &surface, width.max(1) as u32, height.max(1) as u32, density as f64) {
        Ok(engine) => Box::into_raw(Box::new(engine)) as jlong,
        Err(why) => {
            log::error!("the engine could not be made: {why}");
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_destroy(_env: JNIEnv, _this: JObject, handle: jlong) {
    if handle != 0 {
        // Safety: the handle is `create`'s, given back once.
        drop(unsafe { Box::from_raw(handle as *mut AndroidEngine) });
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_resize(_env: JNIEnv, _this: JObject, handle: jlong, width: jint, height: jint) {
    if let Some(engine) = engine(handle) {
        engine.resize(width.max(1) as u32, height.max(1) as u32);
    }
}

/// Draws a frame. Returns the number of tiles drawn, -1 for a frame that failed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_render(_env: JNIEnv, _this: JObject, handle: jlong) -> jint {
    let Some(engine) = engine(handle) else { return -1 };
    match engine.render() {
        Ok(stats) => stats.tiles as jint,
        Err(why) => {
            log::error!("the frame was not drawn: {why}");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_pan(_env: JNIEnv, _this: JObject, handle: jlong, delta_x: jdouble, delta_y: jdouble) {
    if let Some(engine) = engine(handle) {
        engine.pan(delta_x, delta_y);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_endPan(_env: JNIEnv, _this: JObject, handle: jlong) {
    if let Some(engine) = engine(handle) {
        engine.end_pan();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_zoomAbout(_env: JNIEnv, _this: JObject, handle: jlong, scale: jdouble, focus_x: jdouble, focus_y: jdouble) {
    if let Some(engine) = engine(handle) {
        engine.zoom_about(scale, focus_x, focus_y);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_rotate(_env: JNIEnv, _this: JObject, handle: jlong, delta: jdouble) {
    if let Some(engine) = engine(handle) {
        engine.rotate(delta);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_tilt(_env: JNIEnv, _this: JObject, handle: jlong, delta: jdouble) {
    if let Some(engine) = engine(handle) {
        engine.tilt(delta);
    }
}

/// A two-finger tilt drag by a vertical distance in points, down tilting further.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_tiltByDrag(_env: JNIEnv, _this: JObject, handle: jlong, translation_y: jdouble) {
    if let Some(engine) = engine(handle) {
        engine.tilt_by_drag(translation_y);
    }
}

/// The camera at a latitude and longitude in degrees, the bearing and the pitch in radians. Returns 1 when taken.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_artembobkin_immersivemap_NativeEngine_setCameraPosition(_env: JNIEnv, _this: JObject, handle: jlong, latitude: jdouble, longitude: jdouble, zoom: jdouble, bearing: jdouble, pitch: jdouble) -> jint {
    let Some(engine) = engine(handle) else { return 0 };
    match engine.set_camera_position(latitude, longitude, zoom, bearing, pitch) {
        Ok(()) => 1,
        Err(why) => {
            log::warn!("the camera position was not taken: {why}");
            0
        }
    }
}
