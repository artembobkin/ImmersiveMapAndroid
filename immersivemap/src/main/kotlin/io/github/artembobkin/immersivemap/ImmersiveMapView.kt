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
import kotlin.math.abs
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

    private val frameCallback = Choreographer.FrameCallback { frameTimeNanos ->
        frameRequested = false
        if (engine != 0L) {
            // An animation moves the camera to the frame's time, and asks for the next frame while it goes on.
            val animating = NativeEngine.advanceAnimations(engine, frameTimeNanos / NANOS_PER_SECOND) != 0
            val tiles = NativeEngine.render(engine)
            onFrame?.let { listener -> post { listener(tiles) } }
            if (animating) requestFrame()
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
        override fun onDown(event: MotionEvent): Boolean {
            scrolledSinceDown = false
            return true
        }

        /** The second tap's release flies one level in toward it, as on the web, unless the finger dragged. */
        override fun onDoubleTapEvent(event: MotionEvent): Boolean {
            if (event.actionMasked == MotionEvent.ACTION_UP && !scrolledSinceDown) {
                val (x, y) = event.x / density.toDouble() to event.y / density.toDouble()
                val now = System.nanoTime() / NANOS_PER_SECOND
                command { NativeEngine.doubleTapZoom(it, x, y, now) }
            }
            return true
        }

        override fun onScroll(first: MotionEvent?, current: MotionEvent, distanceX: Float, distanceY: Float): Boolean {
            scrolledSinceDown = true
            // Two fingers that may be tilting, or are, don't also pan the map.
            if (current.pointerCount >= 2 && tiltPhase != null && tiltPhase != TiltPhase.REJECTED) return true
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

    /** Whether the finger down now has dragged the map: its release is no tap. */
    private var scrolledSinceDown = false

    /** The angle of the line between the first two pointers, null with fewer than two. */
    private var twoFingerAngle: Double? = null

    /** The fingers' turn so far, until it passes [ROTATION_SLOP] and the map starts turning; null once it has. */
    private var pendingTurn: Double? = 0.0

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        // A touch takes the camera from its animation, where it is.
        if (event.actionMasked == MotionEvent.ACTION_DOWN) command { NativeEngine.cancelAnimations(it) }
        tilt(event)
        pinch.onTouchEvent(event)
        drag.onTouchEvent(event)
        turn(event)
        if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
            command { NativeEngine.endPan(it) }
        }
        return true
    }

    /**
     * Two fingers turning about each other turn the map with them, once they
     * have turned [ROTATION_SLOP] together, as on the web: a pinch's wobble
     * doesn't turn the map.
     */
    private fun turn(event: MotionEvent) {
        if (event.pointerCount < 2 || event.actionMasked == MotionEvent.ACTION_POINTER_UP) {
            twoFingerAngle = null
            if (event.pointerCount < 2) pendingTurn = 0.0
            return
        }
        val angle = atan2((event.getY(1) - event.getY(0)).toDouble(), (event.getX(1) - event.getX(0)).toDouble())
        twoFingerAngle?.let { previous ->
            var delta = angle - previous
            if (delta > PI) delta -= 2 * PI else if (delta < -PI) delta += 2 * PI
            // On a screen whose y runs down, fingers turning clockwise give a
            // positive delta, and a growing bearing turns the map clockwise on
            // screen (the core's RenderCamera::pose): the map turns with them.
            val pending = pendingTurn
            if (pending == null) {
                if (delta != 0.0) command { NativeEngine.rotateByGesture(it, delta) }
            } else if (abs(pending + delta) >= ROTATION_SLOP) {
                pendingTurn = null
                command { NativeEngine.rotateByGesture(it, pending + delta) }
            } else {
                pendingTurn = pending + delta
            }
        }
        twoFingerAngle = angle
    }

    /**
     * Where a two-finger drag stands as a tilt: undecided until the fingers
     * have moved together far enough to tell, then tilting, or rejected and
     * left to the pinch and the turn until the fingers lift. Null with other
     * than two fingers down.
     */
    private enum class TiltPhase { UNDECIDED, TILTING, REJECTED }

    private var tiltPhase: TiltPhase? = null

    /** The two fingers' midpoint at the last event, in points. */
    private var tiltLastX = 0.0
    private var tiltLastY = 0.0

    /** The midpoint's movement not yet applied, in points. */
    private var tiltTranslationX = 0.0
    private var tiltTranslationY = 0.0

    /**
     * Two fingers side by side dragged up or down tilt the camera, down
     * tilting further, as on the other ports (the web's
     * `TwoFingerTiltGestureMath`). Fingers stacked closer to vertically than
     * horizontally are a pinch or a turn and never tilt. The drag commits once
     * the fingers have moved [TILT_ACTIVATION_DISTANCE] together: mostly
     * vertically it tilts, with the movement so far, otherwise it never does.
     */
    private fun tilt(event: MotionEvent) {
        if (event.pointerCount != 2 || event.actionMasked == MotionEvent.ACTION_POINTER_UP ||
            event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL
        ) {
            tiltPhase = null
            return
        }
        val points = density.toDouble()
        val x = (event.getX(0) + event.getX(1)) / 2 / points
        val y = (event.getY(0) + event.getY(1)) / 2 / points
        val phase = tiltPhase
        if (phase == null) {
            val sideBySide = abs(event.getY(1) - event.getY(0)) <= abs(event.getX(1) - event.getX(0))
            tiltPhase = if (sideBySide) TiltPhase.UNDECIDED else TiltPhase.REJECTED
            tiltLastX = x
            tiltLastY = y
            tiltTranslationX = 0.0
            tiltTranslationY = 0.0
            return
        }
        tiltTranslationX += x - tiltLastX
        tiltTranslationY += y - tiltLastY
        tiltLastX = x
        tiltLastY = y
        when (phase) {
            TiltPhase.UNDECIDED -> {
                if (abs(tiltTranslationX) + abs(tiltTranslationY) >= TILT_ACTIVATION_DISTANCE) {
                    if (abs(tiltTranslationY) > abs(tiltTranslationX)) {
                        tiltPhase = TiltPhase.TILTING
                        applyTilt()
                    } else {
                        tiltPhase = TiltPhase.REJECTED
                    }
                }
            }
            TiltPhase.TILTING -> applyTilt()
            TiltPhase.REJECTED -> Unit
        }
    }

    private fun applyTilt() {
        val translationY = tiltTranslationY
        tiltTranslationX = 0.0
        tiltTranslationY = 0.0
        if (translationY != 0.0) command { NativeEngine.tiltByDrag(it, translationY) }
    }

    private companion object {
        /** How far the fingers move together before a two-finger drag is or isn't a tilt, in points. */
        const val TILT_ACTIVATION_DISTANCE = 8.0

        /** How far two fingers turn together before the map turns with them, radians. */
        const val ROTATION_SLOP = 0.06

        const val NANOS_PER_SECOND = 1.0e9
    }
}
