package com.niranjan.helm.ui

import android.annotation.SuppressLint
import android.content.Context
import android.view.MotionEvent
import android.view.View
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import com.niranjan.helm.core.InputFrame
import com.niranjan.helm.core.PenMath
import com.niranjan.helm.core.PenState
import com.niranjan.helm.core.ViewTransform

/**
 * Pen mode on the Screen tab (§13 `pn`): stylus contact, hover, pressure, tilt and the eraser
 * end / side button go to the PC as pen input. Finger touches are ignored (palm rejection).
 */
@Composable
fun PenOverlay(transform: () -> ViewTransform, send: (InputFrame) -> Unit, modifier: Modifier = Modifier) {
    val currentTransform by rememberUpdatedState(transform)
    val currentSend by rememberUpdatedState(send)
    AndroidView(
        factory = { ctx -> PenView(ctx, { currentTransform() }, { currentSend(it) }) },
        modifier = modifier,
    )
}

@SuppressLint("ViewConstructor")
private class PenView(
    context: Context,
    private val transform: () -> ViewTransform,
    private val send: (InputFrame) -> Unit,
) : View(context) {
    private var down = false

    private fun isPen(e: MotionEvent, i: Int = 0): Boolean {
        val t = e.getToolType(i)
        return t == MotionEvent.TOOL_TYPE_STYLUS || t == MotionEvent.TOOL_TYPE_ERASER
    }

    private fun eraser(e: MotionEvent): Boolean =
        e.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER || (e.buttonState and MotionEvent.BUTTON_STYLUS_PRIMARY) != 0

    private fun emit(x: Float, y: Float, pressure: Float, tilt: Float, orientation: Float, state: PenState, eraser: Boolean) {
        val tr = transform()
        if (!tr.isValid) return
        val (nx, ny) = tr.toNormalizedClamped(x, y)
        send(PenMath.frame(nx, ny, pressure, tilt, orientation, state, eraser))
    }

    private fun emitAll(e: MotionEvent, state: PenState) {
        val er = eraser(e)
        for (h in 0 until e.historySize) {
            emit(
                e.getHistoricalX(0, h), e.getHistoricalY(0, h), e.getHistoricalPressure(0, h),
                e.getHistoricalAxisValue(MotionEvent.AXIS_TILT, 0, h), e.getHistoricalOrientation(0, h), state, er,
            )
        }
        emit(e.getX(0), e.getY(0), e.getPressure(0), e.getAxisValue(MotionEvent.AXIS_TILT, 0), e.getOrientation(0), state, er)
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(e: MotionEvent): Boolean {
        if (!isPen(e)) return true // swallow fingers and palms
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                parent?.requestDisallowInterceptTouchEvent(true)
                down = true
                emitAll(e, PenState.DOWN)
            }
            MotionEvent.ACTION_MOVE -> if (down) emitAll(e, PenState.DOWN)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                down = false
                emit(e.getX(0), e.getY(0), 0f, e.getAxisValue(MotionEvent.AXIS_TILT, 0), e.getOrientation(0), PenState.UP, eraser(e))
            }
        }
        return true
    }

    override fun onHoverEvent(e: MotionEvent): Boolean {
        if (!isPen(e) || down) return super.onHoverEvent(e)
        when (e.actionMasked) {
            MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_MOVE -> emitAll(e, PenState.HOVER)
            MotionEvent.ACTION_HOVER_EXIT -> Unit
        }
        return true
    }
}
