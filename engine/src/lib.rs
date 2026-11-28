// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The ImmersiveMap engine for Android. The camera, the presentation and
//! the tile coverage are the shared core's (ImmersiveMapCore), and this
//! crate draws what they resolve with wgpu, over Vulkan on a device.
//!
//! `engine` is free of the platform: it draws into any wgpu surface, or into
//! a texture with none, so it runs and is tested on the machine it is
//! written on. `android` is the binding the Kotlin library calls through JNI.

pub mod engine;
pub mod frame;
pub mod gpu;
pub mod render_layer;
mod tile_grid;

#[cfg(target_os = "android")]
mod android;

pub use engine::{EngineError, FrameStats, MapEngine};
