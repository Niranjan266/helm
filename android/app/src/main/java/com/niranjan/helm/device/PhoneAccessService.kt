package com.niranjan.helm.device

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Color
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.Point
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Display
import android.view.Gravity
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.TextView
import com.niranjan.helm.core.DragPlanner
import com.niranjan.helm.core.FieldSnapshot
import com.niranjan.helm.core.GesturePlanner
import com.niranjan.helm.core.PhoneControl
import com.niranjan.helm.core.PhoneIn
import com.niranjan.helm.core.ScreenBox
import com.niranjan.helm.core.Stroke
import com.niranjan.helm.core.TextPlan
import com.niranjan.helm.core.TextPlanner
import com.niranjan.helm.core.TypedText

/**
 * Performs PC input on this phone, only while [PhoneBridge] says a session is live with control
 * granted. It reads no screen text and keeps no accessibility data: [onAccessibilityEvent] is
 * empty, and on the focused node it looks only at flags and cursor offsets ([FieldSnapshot]),
 * never at the text. Password fields are refused, not typed into.
 */
class PhoneAccessService : AccessibilityService() {
    private val main = Handler(Looper.getMainLooper())
    private val drag = DragPlanner()
    private var gestureRunning = false
    private var queued: Stroke? = null
    private var previous: GestureDescription.StrokeDescription? = null
    private var typed: TypedText? = null
    private var indicator: TextView? = null
    private var secureShown = false
    private val holdCheck = object : Runnable {
        override fun run() {
            if (drag.heldLongerThan(MAX_HOLD_MS, SystemClock.elapsedRealtime())) {
                drag.release()
                pump()
            }
            if (drag.active) main.postDelayed(this, HOLD_CHECK_MS)
        }
    }

    override fun onServiceConnected() {
        instance = this
        running = true
        PhoneBridge.onAccessibilityChanged(true)
    }

    /** Intentionally empty: event data is not looked at, stored or sent. */
    override fun onAccessibilityEvent(event: AccessibilityEvent?) = Unit

    override fun onInterrupt() = Unit

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        gone()
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        gone()
        super.onDestroy()
    }

    private fun gone() {
        if (instance === this) {
            instance = null
            running = false
            hideIndicator()
            PhoneBridge.onAccessibilityChanged(false)
        }
    }

    // ------------------------------------------------------------ input

    /** The real display right now, so rotation needs no extra handling. */
    private fun box(): ScreenBox {
        val size = Point()
        val d = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        @Suppress("DEPRECATION")
        d.getRealSize(size)
        return ScreenBox(size.x, size.y)
    }

    fun perform(ev: PhoneIn) {
        when (ev) {
            is PhoneIn.Tap -> single(GesturePlanner.tap(box(), ev.x, ev.y))
            is PhoneIn.LongPress -> single(GesturePlanner.longPress(box(), ev.x, ev.y))
            is PhoneIn.Swipe -> single(GesturePlanner.swipe(box(), ev))
            is PhoneIn.Scroll -> GesturePlanner.scroll(box(), ev)?.let { single(it) }
            is PhoneIn.Down -> {
                drag.down(box().px(ev.x, ev.y), SystemClock.elapsedRealtime())
                main.removeCallbacks(holdCheck)
                main.postDelayed(holdCheck, HOLD_CHECK_MS)
                pump()
            }
            is PhoneIn.Move -> {
                drag.move(box().px(ev.x, ev.y))
                pump()
            }
            is PhoneIn.Up -> {
                drag.up(box().px(ev.x, ev.y))
                pump()
            }
            is PhoneIn.Global -> performGlobalAction(
                when (ev.action) {
                    PhoneIn.GlobalAction.BACK -> GLOBAL_ACTION_BACK
                    PhoneIn.GlobalAction.HOME -> GLOBAL_ACTION_HOME
                    PhoneIn.GlobalAction.RECENTS -> GLOBAL_ACTION_RECENTS
                    PhoneIn.GlobalAction.NOTIFICATIONS -> GLOBAL_ACTION_NOTIFICATIONS
                },
            )
            is PhoneIn.Text -> typeText(ev.text)
            is PhoneIn.Key -> when (ev.key) {
                PhoneIn.EditKey.BACKSPACE -> backspace()
                PhoneIn.EditKey.ENTER -> enter()
            }
        }
    }

    private fun pathOf(s: Stroke): Path = Path().apply {
        val first = s.points.first()
        moveTo(first.x, first.y)
        // A lone point still needs a segment; a zero-length path counts as a tap.
        if (s.points.size == 1) lineTo(first.x, first.y)
        for (p in s.points.drop(1)) lineTo(p.x, p.y)
    }

    /** One complete gesture (tap, long press, swipe, scroll); a running drag is left alone. */
    private fun single(s: Stroke) {
        if (drag.active) return
        if (gestureRunning) {
            // Android would cancel the running one; the newest waits its turn instead.
            queued = s
            return
        }
        val stroke = GestureDescription.StrokeDescription(pathOf(s), 0, s.durationMs, false)
        dispatch(GestureDescription.Builder().addStroke(stroke).build(), keep = null)
    }

    /** Next piece of a live drag, once the piece before it has finished. */
    private fun pump() {
        if (gestureRunning) return
        queued?.let {
            queued = null
            single(it)
            return
        }
        val s = drag.next(SystemClock.elapsedRealtime()) ?: return
        val stroke = if (s.continues && previous != null) {
            previous!!.continueStroke(pathOf(s), 0, s.durationMs, s.willContinue)
        } else {
            GestureDescription.StrokeDescription(pathOf(s), 0, s.durationMs, s.willContinue)
        }
        dispatch(GestureDescription.Builder().addStroke(stroke).build(), keep = stroke.takeIf { s.willContinue })
    }

    private fun dispatch(g: GestureDescription, keep: GestureDescription.StrokeDescription?) {
        gestureRunning = true
        previous = keep
        val started = runCatching {
            dispatchGesture(
                g,
                object : GestureResultCallback() {
                    override fun onCompleted(gestureDescription: GestureDescription?) {
                        gestureRunning = false
                        pump()
                    }

                    override fun onCancelled(gestureDescription: GestureDescription?) {
                        gestureRunning = false
                        previous = null
                        queued = null
                        drag.forget()
                    }
                },
                main,
            )
        }.getOrDefault(false)
        if (!started) {
            gestureRunning = false
            previous = null
            drag.forget()
        }
    }

    /** Session over: nothing may stay pressed. */
    fun releaseAll() {
        typed = null
        queued = null
        drag.release()
        pump()
    }

    // ------------------------------------------------------------ text

    private fun focused(): AccessibilityNodeInfo? = runCatching { findFocus(AccessibilityNodeInfo.FOCUS_INPUT) }.getOrNull()

    /** Flags and offsets only. The node's text is never touched. */
    private fun snapshot(n: AccessibilityNodeInfo?): FieldSnapshot? = n?.let {
        FieldSnapshot(
            nodeKey = it.hashCode(),
            editable = it.isEditable,
            password = it.isPassword,
            selectionStart = it.textSelectionStart,
            selectionEnd = it.textSelectionEnd,
            showingHint = it.isShowingHintText,
        )
    }

    private fun runPlan(node: AccessibilityNodeInfo?, f: FieldSnapshot?, plan: TextPlan) {
        val blocked = plan is TextPlan.Blocked
        if (blocked != secureShown) {
            secureShown = blocked
            PhoneBridge.reportSecure(if (blocked) "password" else null)
        }
        when (plan) {
            is TextPlan.SetText -> {
                val n = node ?: return
                val ok = n.performAction(
                    AccessibilityNodeInfo.ACTION_SET_TEXT,
                    Bundle().apply {
                        putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, plan.value)
                    },
                )
                if (ok) {
                    typed = TypedText(f!!.nodeKey, plan.value)
                    select(n, plan.value.length, plan.value.length)
                } else {
                    typed = null
                }
            }
            is TextPlan.Paste -> {
                val n = node ?: return
                typed = null
                setClip(plan.text)
                n.performAction(AccessibilityNodeInfo.ACTION_PASTE)
                clearClipSoon()
            }
            is TextPlan.CutBefore -> {
                val n = node ?: return
                typed = null
                if (select(n, plan.selectionEnd - 1, plan.selectionEnd)) {
                    n.performAction(AccessibilityNodeInfo.ACTION_CUT)
                    clearClipSoon()
                }
            }
            TextPlan.NoField, TextPlan.Blocked, TextPlan.Skip -> typed = typed.takeIf { plan is TextPlan.Skip }
        }
    }

    private fun typeText(text: String) {
        val node = focused()
        val f = snapshot(node)
        runPlan(node, f, TextPlanner.text(f, text, typed))
    }

    private fun backspace() {
        val node = focused()
        val f = snapshot(node)
        runPlan(node, f, TextPlanner.backspace(f, typed))
    }

    private fun enter() {
        val node = focused() ?: return
        val f = snapshot(node) ?: return
        if (f.password) return runPlan(node, f, TextPlan.Blocked)
        if (!f.editable) return
        typed = null
        if (Build.VERSION.SDK_INT >= 30) {
            val action = AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER
            if (node.actionList.contains(action) && node.performAction(action.id)) return
        }
        runPlan(node, f, TextPlan.Paste("\n"))
    }

    private fun select(n: AccessibilityNodeInfo, start: Int, end: Int): Boolean = n.performAction(
        AccessibilityNodeInfo.ACTION_SET_SELECTION,
        Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, start)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, end)
        },
    )

    private fun clipboard() = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager

    private fun setClip(text: String) {
        runCatching { clipboard().setPrimaryClip(ClipData.newPlainText("Helm", text)) }
    }

    /** What the PC typed does not stay on the clipboard (it is overwritten either way). */
    private fun clearClipSoon() {
        if (Build.VERSION.SDK_INT >= 28) main.postDelayed({ runCatching { clipboard().clearPrimaryClip() } }, 1_000)
    }

    // ------------------------------------------------------------ indicator

    /** The visible "PC is controlling" pill. An accessibility overlay needs no extra permission. */
    fun showIndicator(pcName: String, control: Boolean) {
        val text = if (control) "PC is controlling  ·  Stop" else "PC can see this  ·  Stop"
        indicator?.let {
            it.text = text
            return
        }
        val view = TextView(this).apply {
            this.text = text
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(230, 196, 30, 30))
            setPadding(24, 10, 24, 10)
            textSize = 12f
            contentDescription = "$pcName is connected. Tap to stop."
            setOnClickListener { PhoneScreenService.current?.finishSession(PhoneControl.REASON_STOPPED, tellPc = true) }
        }
        val lp = WindowManager.LayoutParams(
            WindowManager.LayoutParams.WRAP_CONTENT,
            WindowManager.LayoutParams.WRAP_CONTENT,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL,
            PixelFormat.TRANSLUCENT,
        ).apply { gravity = Gravity.TOP or Gravity.CENTER_HORIZONTAL }
        if (runCatching { getSystemService(WindowManager::class.java).addView(view, lp) }.isSuccess) indicator = view
    }

    fun hideIndicator() {
        releaseAll()
        secureShown = false
        val v = indicator ?: return
        indicator = null
        runCatching { getSystemService(WindowManager::class.java).removeView(v) }
    }

    companion object {
        /** Lift a finger the PC has said nothing about for this long. */
        private const val MAX_HOLD_MS = 30_000L
        private const val HOLD_CHECK_MS = 2_000L

        @Volatile
        var instance: PhoneAccessService? = null
            private set

        @Volatile
        var running = false
            private set
    }
}
