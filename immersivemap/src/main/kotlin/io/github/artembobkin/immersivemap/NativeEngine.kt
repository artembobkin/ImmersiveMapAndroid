// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

package io.github.artembobkin.immersivemap

import android.view.Surface

/**
 * The engine, which is Rust (`engine/src/android.rs`). An engine is made
 * over a `Surface` and named by a handle, 0 for none. Every call for one
 * engine comes from one thread, the view's render thread: the engine does
 * not lock.
 */
internal object NativeEngine {
    init {
        System.loadLibrary("immersive_map_engine")
    }

    /** An engine drawing to [surface], [width] by [height] pixels at [density] pixels a point. 0 when it could not be made. */
    external fun create(surface: Surface, width: Int, height: Int, density: Float): Long

    external fun destroy(handle: Long)

    external fun resize(handle: Long, width: Int, height: Int)

    /** Draws a frame. The number of tiles drawn, -1 for a frame that failed. */
    external fun render(handle: Long): Int

    /** A drag by a distance in points. */
    external fun pan(handle: Long, deltaX: Double, deltaY: Double)

    external fun endPan(handle: Long)

    /** A pinch by [scale] about a point of the view, in points from its top left. */
    external fun zoomAbout(handle: Long, scale: Double, focusX: Double, focusY: Double)

    /** A turn of the map by an angle, radians. */
    external fun rotate(handle: Long, delta: Double)

    /** A tilt of the camera by an angle, radians. */
    external fun tilt(handle: Long, delta: Double)

    /** The camera at a latitude and longitude in degrees, the bearing and the pitch in radians. 1 when taken. */
    external fun setCameraPosition(handle: Long, latitude: Double, longitude: Double, zoom: Double, bearing: Double, pitch: Double): Int
}
