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

#[test]
fn a_two_finger_drag_down_tilts_and_up_levels() {
    use immersive_map_engine::engine::{TILT_GESTURE_SENSITIVITY, tilt_drag_pitch_delta};
    // The full height sweeps the reachable pitch twice, as on the web.
    assert!((tilt_drag_pitch_delta(960.0, 1920.0, 1.2, TILT_GESTURE_SENSITIVITY) - 1.2).abs() < 1e-12);
    assert!((tilt_drag_pitch_delta(-480.0, 1920.0, 1.2, TILT_GESTURE_SENSITIVITY) + 0.6).abs() < 1e-12);
    assert_eq!(tilt_drag_pitch_delta(100.0, 0.0, 1.2, TILT_GESTURE_SENSITIVITY), 0.0, "no view, no tilt");

    let Some(mut engine) = engine() else { return };
    let level = engine.camera_state().pitch;
    engine.tilt_by_drag(200.0);
    let tilted = engine.camera_state().pitch;
    assert!(tilted > level, "down tilts further");
    engine.tilt_by_drag(-100.0);
    let back = engine.camera_state().pitch;
    assert!(back < tilted && back >= level, "up levels off");
    engine.tilt_by_drag(1.0e6);
    let most = engine.camera_state().pitch;
    engine.tilt_by_drag(10.0);
    assert_eq!(engine.camera_state().pitch, most, "held at the reachable pitch");
    validated_frame(&mut engine);
}

#[test]
fn a_double_tap_flies_one_level_in_toward_the_point() {
    use immersive_map_engine::engine::{DOUBLE_TAP_FLIGHT_DURATION, DOUBLE_TAP_ZOOM_DELTA};
    let Some(mut engine) = engine() else { return };
    let start = engine.camera_state();
    // Where a pinch of the same zoom about the same point lands is where the flight lands.
    engine.zoom_about(DOUBLE_TAP_ZOOM_DELTA.exp2(), 200.0, 300.0);
    let expected = engine.camera_state();
    engine.set_camera_position(55.751244, 37.618423, 12.0, 0.0, 0.0).expect("the start");
    assert_eq!(engine.camera_state(), start);

    // From time 0, the duration is exact in floating point.
    assert!(engine.double_tap_zoom(200.0, 300.0, 0.0));
    assert_eq!(engine.camera_state(), start, "the flight starts where the camera is");
    assert!(engine.advance_animations(DOUBLE_TAP_FLIGHT_DURATION / 2.0), "half way it goes on");
    let half = engine.camera_state();
    assert!(half.zoom > start.zoom && half.zoom < expected.zoom);
    validated_frame(&mut engine);
    assert!(!engine.advance_animations(DOUBLE_TAP_FLIGHT_DURATION), "it lands at its duration");
    let landed = engine.camera_state();
    assert!((landed.zoom - start.zoom - DOUBLE_TAP_ZOOM_DELTA).abs() < 1e-9);
    for axis in 0..2 {
        assert!((landed.center_world_mercator[axis] - expected.center_world_mercator[axis]).abs() < 1e-12, "the point stays under the finger");
    }
    assert!(!engine.advance_animations(11.0), "nothing goes on after it");

    // A touch stops the flight where it is.
    assert!(engine.double_tap_zoom(200.0, 300.0, 20.0));
    engine.advance_animations(20.1);
    let stopped = engine.camera_state();
    engine.cancel_animations();
    assert!(!engine.advance_animations(20.3));
    assert_eq!(engine.camera_state(), stopped);
}

#[test]
fn a_two_finger_turn_turns_the_map_by_the_sensitivity() {
    use immersive_map_engine::engine::ROTATION_GESTURE_SENSITIVITY;
    let Some(mut engine) = engine() else { return };
    let before = engine.camera_state().bearing;
    engine.rotate_by_gesture(0.5);
    let turned = engine.camera_state().bearing - before;
    assert!(turned > 0.0, "clockwise fingers turn the map clockwise");
    assert!((turned - 0.5 * ROTATION_GESTURE_SENSITIVITY).abs() < 1e-12);
}
