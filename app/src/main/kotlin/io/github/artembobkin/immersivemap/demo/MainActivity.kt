// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

package io.github.artembobkin.immersivemap.demo

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import io.github.artembobkin.immersivemap.ImmersiveMap
import io.github.artembobkin.immersivemap.rememberImmersiveMapState

/** The app the work in progress is run in: the map full screen, the frame's tile count and the tilt over it. */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val map = rememberImmersiveMapState()
            var tiles by remember { mutableIntStateOf(0) }
            Box(Modifier.fillMaxSize()) {
                ImmersiveMap(Modifier.fillMaxSize(), map, onFrame = { tiles = it })
                Row(
                    Modifier
                        .align(Alignment.BottomCenter)
                        .safeDrawingPadding()
                        .padding(16.dp)
                        .background(Color(0xCCFFFFFF), RoundedCornerShape(16.dp))
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("$tiles tiles")
                    FilledTonalButton(onClick = { map.tiltBy(0.15) }) { Text("Tilt") }
                    FilledTonalButton(onClick = { map.tiltBy(-0.15) }) { Text("Level") }
                    FilledTonalButton(onClick = { map.setCameraPosition(40.7128, -74.0060, 14.0, 0.4, 1.0) }) { Text("New York") }
                }
            }
        }
    }
}
