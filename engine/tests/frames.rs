// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

//! The engine drawing to a texture on the machine the tests run on: the
//! pipeline is built and frames are drawn with no validation error, and the
//! gestures move the camera and the coverage with it. A machine with no GPU
//! skips them.

use immersive_map_engine::{EngineError, MapEngine};

fn engine() -> Option<MapEngine> {
    match pollster::block_on(MapEngine::offscreen(1080, 1920)) {
        Ok(engine) => Some(engine),
        Err(EngineError::NoAdapter(why)) => {
            eprintln!("no GPU adapter, the frame tests are skipped: {why}");
            None
        }
        Err(error) => panic!("{error}"),
    }
}

/// A frame drawn inside a validation scope: a pipeline, a buffer or a pass that is wrong fails the test.
fn validated_frame(engine: &mut MapEngine) -> immersive_map_engine::FrameStats {
    let scope = engine.device().push_error_scope(wgpu::ErrorFilter::Validation);
    let stats = engine.render().expect("a frame");
    if let Some(error) = pollster::block_on(scope.pop()) {
        panic!("the frame is not valid: {error}");
    }
    stats
}

#[test]
fn a_frame_draws_the_coverage_of_its_camera() {
    let Some(mut engine) = engine() else { return };
    let stats = validated_frame(&mut engine);
    assert_eq!(stats.target_zoom, 12);
    assert!(stats.tiles > 0, "the camera over Moscow sees tiles");
    // The same camera draws the same coverage.
    assert_eq!(validated_frame(&mut engine).coverage_version, stats.coverage_version);
}

#[test]
fn gestures_move_the_camera_and_the_coverage_follows() {
    let Some(mut engine) = engine() else { return };
    let first = validated_frame(&mut engine);
    let before = engine.camera_state();
    engine.pan(-400.0, 0.0);
    engine.end_pan();
    let panned = engine.camera_state();
    assert!(panned.center_world_mercator[0] > before.center_world_mercator[0], "dragging left brings the east into view");
    engine.zoom_about(4.0, 100.0, 100.0);
    let zoomed = engine.camera_state();
    assert!((zoomed.zoom - 14.0).abs() < 1e-9, "a pinch of four is two levels");
    assert!(zoomed.center_world_mercator[0] < panned.center_world_mercator[0] && zoomed.center_world_mercator[1] < panned.center_world_mercator[1], "toward the pinch's point, up and left of the centre");
    engine.tilt(1.0);
    engine.rotate(0.7);
    let stats = validated_frame(&mut engine);
    assert_eq!(stats.target_zoom, 14);
    assert_ne!(stats.coverage_version, first.coverage_version);
    assert!(stats.tiles > first.tiles, "a tilted camera sees the far bands too");
    engine.resize(1920, 1080);
    validated_frame(&mut engine);
    assert!(engine.set_camera_position(89.0, 0.0, 3.0, 0.0, 0.0).is_err(), "past the Mercator limit");
}
