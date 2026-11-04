package com.niranjan.helm.core

/** Where finished HID reports go. Implementations must not block: queue and return. */
fun interface HidReportSink {
    fun send(reportId: Int, data: ByteArray)
}

/**
 * Turns the phone's touch, key and text input into HID reports. It speaks the same
 * [TouchPadSink] as the Wi-Fi connection, so the touchpad, the floating keyboard and the type
 * box work unchanged. Motion and scroll are summed and sent as one mouse report at most every
 * [minIntervalMs]; anything held back stays summed for the next flush. Every method is
 * thread-safe.
 */
class BtInputBridge(
    private val out: HidReportSink,
    private val clock: () -> Long = { System.nanoTime() / 1_000_000L },
    private val minIntervalMs: Long = 8L,
) : TouchPadSink {
    private val motion = DeltaAccumulator(MouseReport.MAX_XY)
    private val wheel = DeltaAccumulator(MouseReport.MAX_WHEEL)
    private var buttons = 0
    private var heldMods = 0
    private val heldKeys = ArrayList<Int>()
    private var consumerHeld = 0
    private var dragLocked = false
    private var lastMouseAt = Long.MIN_VALUE / 2
    private var skipped = 0

    /** Characters typed that have no US-layout key; they were left out. */
    val skippedChars: Int get() = synchronized(this) { skipped }

    val isDragLocked: Boolean get() = synchronized(this) { dragLocked }

    // ---- Pointer ------------------------------------------------------------------------

    @Synchronized
    override fun touchMove(dx: Float, dy: Float) = motion.add(dx, dy)

    /** Scroll notches: `dx` is the horizontal pan, `dy` the wheel (positive = up). */
    @Synchronized
    override fun touchScroll(dx: Float, dy: Float) = wheel.add(dx, dy)

    @Synchronized
    override fun flushInput() = flushMouse(force = false)

    private fun flushMouse(force: Boolean) {
        val now = clock()
        if (!force && now - lastMouseAt < minIntervalMs) return
        val m = motion.drain()
        val w = wheel.drain()
        if (m == null && w == null) return
        lastMouseAt = now
        // The wheel accumulator was fed (dx, dy) = (pan, wheel).
        out.send(HidIds.MOUSE, MouseReport.build(buttons, m?.first ?: 0, m?.second ?: 0, w?.second ?: 0, w?.first ?: 0))
    }

    private fun sendButtons() {
        out.send(HidIds.MOUSE, MouseReport.build(buttons, 0, 0))
    }

    @Synchronized
    override fun click(button: MouseButton) {
        flushMouse(force = true)
        if (dragLocked && button == MouseButton.LEFT) {
            setDragLockLocked(false)
            return
        }
        setButton(button, true)
        setButton(button, false)
    }

    @Synchronized
    fun setDragLock(on: Boolean): Boolean {
        if (on == dragLocked) return dragLocked
        flushMouse(force = true)
        setDragLockLocked(on)
        return dragLocked
    }

    private fun setDragLockLocked(on: Boolean) {
        setButton(MouseButton.LEFT, on)
        dragLocked = on
    }

    private fun setButton(button: MouseButton, down: Boolean) {
        val bit = MouseReport.buttonBit(button)
        val next = if (down) buttons or bit else buttons and bit.inv()
        if (next == buttons) return
        buttons = next
        sendButtons()
    }

    @Synchronized
    override fun pinch(start: Boolean) {
        flushMouse(force = true)
        if (start) pressVk(Vk.CONTROL) else releaseVk(Vk.CONTROL)
    }

    // ---- Keyboard -----------------------------------------------------------------------

    @Synchronized
    override fun sendAll(frames: List<InputFrame>) {
        for (f in frames) apply(f)
    }

    private fun apply(f: InputFrame) {
        when (f) {
            is InputFrame.Move -> motion.add(f.dx, f.dy)
            is InputFrame.Gyro -> motion.add(f.dx, f.dy)
            is InputFrame.Scroll -> wheel.add(f.dx, f.dy)
            is InputFrame.Button -> {
                flushMouse(force = true)
                setButton(f.button, f.down)
            }
            is InputFrame.KeyDown -> {
                flushMouse(force = true)
                pressVk(f.vk)
            }
            is InputFrame.KeyUp -> releaseVk(f.vk)
            is InputFrame.Shortcut -> {
                flushMouse(force = true)
                f.keys.forEach { pressVk(it) }
                f.keys.asReversed().forEach { releaseVk(it) }
            }
            is InputFrame.Text -> typeText(f.text)
            // Absolute moves, pen and laser frames only make sense with Helm on the PC.
            else -> Unit
        }
    }

    /** Types [text] on a US layout; characters without a key are skipped and counted. */
    @Synchronized
    fun typeText(text: String) {
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            i += Character.charCount(cp)
            val stroke = HidKeys.strokeForChar(cp)
            if (stroke == null) {
                skipped++
                continue
            }
            sendKeys(heldMods or stroke.modifiers, heldKeys + stroke.usage)
            sendKeys(heldMods, heldKeys)
        }
    }

    private fun pressVk(vk: Int) {
        val bit = HidKeys.modifierBit(vk)
        if (bit != 0) {
            heldMods = heldMods or bit
        } else {
            val u = HidKeys.usageForVk(vk) ?: return
            if (u !in heldKeys && heldKeys.size < KeyboardReport.ROLLOVER) heldKeys += u
        }
        sendKeys(heldMods, heldKeys)
    }

    private fun releaseVk(vk: Int) {
        val bit = HidKeys.modifierBit(vk)
        if (bit != 0) {
            heldMods = heldMods and bit.inv()
        } else {
            val u = HidKeys.usageForVk(vk) ?: return
            if (!heldKeys.remove(u)) return
        }
        sendKeys(heldMods, heldKeys)
    }

    private fun sendKeys(mods: Int, usages: List<Int>) {
        out.send(HidIds.KEYBOARD, KeyboardReport.build(mods, usages))
    }

    // ---- Media --------------------------------------------------------------------------

    @Synchronized
    fun pressConsumer(key: ConsumerKey) {
        consumerHeld = consumerHeld or key.bit
        out.send(HidIds.CONSUMER, ConsumerReport.build(consumerHeld))
        consumerHeld = consumerHeld and key.bit.inv()
        out.send(HidIds.CONSUMER, ConsumerReport.build(consumerHeld))
    }

    // ---- Release ------------------------------------------------------------------------

    /**
     * Lets go of every button, key and media key (sends the all-up reports) and forgets
     * pending motion. Call when leaving, disconnecting or going to the background.
     */
    @Synchronized
    fun releaseAll() {
        resetLocal()
        out.send(HidIds.MOUSE, MouseReport.build(0, 0, 0))
        out.send(HidIds.KEYBOARD, KeyboardReport.build(0, emptyList()))
        out.send(HidIds.CONSUMER, ConsumerReport.build(0))
    }

    /** Forgets everything without sending (the link is already gone). */
    @Synchronized
    fun resetLocal() {
        motion.clear()
        wheel.clear()
        buttons = 0
        heldMods = 0
        heldKeys.clear()
        consumerHeld = 0
        dragLocked = false
    }

    @Synchronized
    fun clearSkipped() {
        skipped = 0
    }
}
