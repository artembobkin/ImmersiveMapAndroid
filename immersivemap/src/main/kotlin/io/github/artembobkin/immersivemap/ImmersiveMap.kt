// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

package io.github.artembobkin.immersivemap

import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView

/**
 * What an app holds to command a map it shows with [ImmersiveMap]. A command
 * given before the map is on screen is kept and applied when it is.
 */
@Stable
class ImmersiveMapState internal constructor() {
    private var view: ImmersiveMapView? = null
    private var pendingCamera: DoubleArray? = null

    /** Puts the camera at a latitude and longitude in degrees, the bearing and the pitch in radians. */
    fun setCameraPosition(latitude: Double, longitude: Double, zoom: Double, bearing: Double = 0.0, pitch: Double = 0.0) {
        val current = view
        if (current == null) {
            pendingCamera = doubleArrayOf(latitude, longitude, zoom, bearing, pitch)
        } else {
            current.setCameraPosition(latitude, longitude, zoom, bearing, pitch)
        }
    }

    /** Tilts the camera by an angle in radians. */
    fun tiltBy(radians: Double) {
        view?.tiltBy(radians)
    }

    internal fun attach(view: ImmersiveMapView) {
        this.view = view
        pendingCamera?.let { view.setCameraPosition(it[0], it[1], it[2], it[3], it[4]) }
        pendingCamera = null
    }

    internal fun detach() {
        view = null
    }
}

@Composable
fun rememberImmersiveMapState(): ImmersiveMapState = remember { ImmersiveMapState() }

/**
 * The map. [onFrame] is called after every frame with the number of tiles it
 * drew: frames are drawn on demand, so it is quiet while nothing moves.
 */
@Composable
fun ImmersiveMap(
    modifier: Modifier = Modifier,
    state: ImmersiveMapState = rememberImmersiveMapState(),
    onFrame: ((tiles: Int) -> Unit)? = null,
) {
    AndroidView(
        modifier = modifier,
        factory = { context -> ImmersiveMapView(context).also(state::attach) },
        update = { view -> view.onFrame = onFrame },
        onRelease = { state.detach() },
    )
}
