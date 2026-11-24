// Copyright (c) 2025-2026 Artem Bobkin.
// SPDX-License-Identifier: MIT

package io.github.artembobkin.immersivemap

import android.annotation.SuppressLint
import android.content.Context
import android.os.Handler
import android.os.HandlerThread
import android.util.AttributeSet
import android.view.Choreographer
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.SurfaceHolder
import android.view.SurfaceView
import java.util.concurrent.CountDownLatch
import kotlin.math.PI
import kotlin.math.atan2

/**
 * The map as a view. It owns a `Surface` the system composes on its own
 * (a [SurfaceView], no copy through the view hierarchy), a render thread
 * that is the only one to call the engine, and the gestures.
 *
 * Rendering is on demand: nothing is drawn until something asks for a
 * frame (a gesture, a camera command, the surface changing), and then one
 * frame is drawn at the display's next vsync. A change that should show
 * must call [requestFrame].
 */
class ImmersiveMapView @JvmOverloads constructor(context: Context, attrs: AttributeSet? = null) :
    SurfaceView(context, attrs), SurfaceHolder.Callback {

    /** Called on the main thread after a frame, with the number of tiles it drew. */
    var onFrame: ((tiles: Int) -> Unit)? = null

    private val renderThread = HandlerThread("ImmersiveMapRender").apply { start() }
    private val render = Handler(renderThread.looper)

    /** The engine's handle, 0 with no surface. Read and written on the render thread only. */
    private var engine = 0L
    private var frameRequested = false
    private val density = resources.displayMetrics.density

    /** The camera to start at, kept until there is an engine to give it to. */
    private var pendingCamera: DoubleArray? = null

    private val frameCallback = Choreographer.FrameCallback {
        frameRequested = false
        if (engine != 0L) {
            val tiles = NativeEngine.render(engine)
            onFrame?.let { listener -> post { listener(tiles) } }
        }
    }

    init {
        holder.addCallback(this)
    }

    /** Asks for one frame at the next vsync. Any thread. */
    fun requestFrame() {
        render.post {
            if (!frameRequested && engine != 0L) {
                frameRequested = true
                // The render thread's own choreographer: the frame is drawn on this thread.
                Choreographer.getInstance().postFrameCallback(frameCallback)
            }
        }
    }

    /** Puts the camera at a latitude and longitude in degrees, the bearing and the pitch in radians. */
    fun setCameraPosition(latitude: Double, longitude: Double, zoom: Double, bearing: Double = 0.0, pitch: Double = 0.0) {
        val camera = doubleArrayOf(latitude, longitude, zoom, bearing, pitch)
        render.post {
            if (engine == 0L) {
                pendingCamera = camera
            } else {
                NativeEngine.setCameraPosition(engine, camera[0], camera[1], camera[2], camera[3], camera[4])
            }
        }
        requestFrame()
    }

    /** Tilts the camera by an angle in radians, within the engine's range. */
    fun tiltBy(radians: Double) = command { NativeEngine.tilt(it, radians) }

    /** Runs a call on the render thread against the engine, when there is one, and asks for a frame. */
    private fun command(call: (Long) -> Unit) {
        render.post { if (engine != 0L) call(engine) }
        requestFrame()
    }

    // The surface

    override fun surfaceCreated(holder: SurfaceHolder) {
        val surface = holder.surface
        val (width, height) = holder.surfaceFrame.let { it.width() to it.height() }
        render.post {
            engine = NativeEngine.create(surface, width, height, density)
            pendingCamera?.let { camera ->
                if (engine != 0L) NativeEngine.setCameraPosition(engine, camera[0], camera[1], camera[2], camera[3], camera[4])
                pendingCamera = null
            }
        }
        requestFrame()
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        command { NativeEngine.resize(it, width, height) }
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        // The surface is gone when this returns: the engine that draws to it is destroyed first.
        val done = CountDownLatch(1)
        render.post {
            Choreographer.getInstance().removeFrameCallback(frameCallback)
            frameRequested = false
            if (engine != 0L) {
                NativeEngine.destroy(engine)
                engine = 0L
            }
            done.countDown()
        }
        done.await()
    }

    override fun onDetachedFromWindow() {
        super.onDetachedFromWindow()
        renderThread.quitSafely()
    }

    // The gestures

    private val drag = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(event: MotionEvent) = true

        override fun onScroll(first: MotionEvent?, current: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
            // The distances are from the current event back to the last: the finger's move is their negative.
            val (deltaX, deltaY) = -distanceX / density.toDouble() to -distanceY / density.toDouble()
            command { NativeEngine.pan(it, deltaX, deltaY) }
            return true
        }
    })

    private val pinch = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScale(detector: ScaleGestureDetector): Boolean {
            val scale = detector.scaleFactor.toDouble()
            val (focusX, focusY) = detector.focusX / density.toDouble() to detector.focusY / density.toDouble()
            command { NativeEngine.zoomAbout(it, scale, focusX, focusY) }
            return true
        }
    })

    /** The angle of the line between the first two pointers, null with fewer than two. */
    private var twoFingerAngle: Double? = null

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        pinch.onTouchEvent(event)
        drag.onTouchEvent(event)
        turn(event)
        if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
            command { NativeEngine.endPan(it) }
        }
        return true
    }

    /** Two fingers turning about each other turn the map with them. */
    private fun turn(event: MotionEvent) {
        if (event.pointerCount < 2 || event.actionMasked == MotionEvent.ACTION_POINTER_UP) {
            twoFingerAngle = null
            return
        }
        val angle = atan2((event.getY(1) - event.getY(0)).toDouble(), (event.getX(1) - event.getX(0)).toDouble())
        twoFingerAngle?.let { previous ->
            var delta = angle - previous
            if (delta > PI) delta -= 2 * PI else if (delta < -PI) delta += 2 * PI
            // The screen's y runs down, the map's bearing is counted the other way round.
            if (delta != 0.0) command { NativeEngine.rotate(it, -delta) }
        }
        twoFingerAngle = angle
    }
}
