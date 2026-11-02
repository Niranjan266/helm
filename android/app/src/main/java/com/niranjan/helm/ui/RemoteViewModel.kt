package com.niranjan.helm.ui

import android.app.Application
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.AppEntry
import com.niranjan.helm.core.AudioState
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.DeckButton
import com.niranjan.helm.core.DockCodec
import com.niranjan.helm.core.DockTile
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmEvent
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.InputFrame
import com.niranjan.helm.core.MediaState
import com.niranjan.helm.core.MonitorInfo
import com.niranjan.helm.core.MotionBatcher
import com.niranjan.helm.core.MouseButton
import com.niranjan.helm.core.PowerSchedule
import com.niranjan.helm.core.ScrollBatcher
import com.niranjan.helm.core.Vk
import com.niranjan.helm.core.WindowEntry
import com.niranjan.helm.core.DisplayBrightness
import com.niranjan.helm.core.CustomIcon
import com.niranjan.helm.core.DeckLayout
import com.niranjan.helm.core.DeckPage
import com.niranjan.helm.core.DeckPages
import com.niranjan.helm.core.DockFolders
import com.niranjan.helm.core.SmartOrder
import com.niranjan.helm.core.appUsage
import com.niranjan.helm.core.deck2
import com.niranjan.helm.core.dock2
import com.niranjan.helm.core.profiles
import com.niranjan.helm.core.putIcon
import com.niranjan.helm.core.setDeck2
import com.niranjan.helm.core.supportsG5
import com.niranjan.helm.device.SharedItem
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope

data class TransferUi(val label: String, val progress: Float?)

data class RemoteUi(
    val phase1: Boolean = true,
    val dock: List<List<DockTile>> = emptyList(),
    val apps: List<AppEntry>? = null,
    val appsLoading: Boolean = false,
    val windows: List<WindowEntry> = emptyList(),
    val media: MediaState? = null,
    /** When [media] was received (for extrapolating the position while playing). */
    val mediaAtMs: Long = 0,
    val artwork: Bitmap? = null,
    val audio: AudioState? = null,
    val displays: List<DisplayBrightness> = emptyList(),
    val power: PowerSchedule? = null,
    val deck: List<DeckButton> = emptyList(),
    val monitors: List<MonitorInfo> = emptyList(),
    val screenshot: Bitmap? = null,
    val screenshotPng: ByteArray? = null,
    val capturing: Boolean = false,
    val transfer: TransferUi? = null,
    val clipboardWatch: Boolean = false,
    val pcClipImage: Bitmap? = null,
    /** Agent advertises Phase 3 (`p3`). */
    val phase3: Boolean = false,
    /** Agent advertises Phase 4 (`p4`). */
    val phase4: Boolean = false,
    /** Phase 5 extras (§13). */
    val phase5: Boolean = false,
    /** `deck.state{id,on}` per button. */
    val deckOn: Map<String, Boolean> = emptyMap(),
    /** `deck.widget{id,text}` per button. */
    val deckText: Map<String, String> = emptyMap(),
    /** G5 (dock-deck-2.md): agent has dock folders, deck pages, usage and custom icons. */
    val g5: Boolean = false,
    /** G5 deck pages (always at least one); [deck] holds every button. */
    val deckPages: List<DeckPage> = listOf(DeckPage(DeckPages.DEFAULT_ID, "Main", emptyList())),
    /** G5 usage scores by lowercase app id (smart order). */
    val usage: Map<String, Double> = emptyMap(),
    /** Deck page the active profile asks for; DeckScreen switches to it once. */
    val deckPageWanted: String? = null,
) {
    val deckLayout: DeckLayout get() = DeckLayout(deck, deckPages)
}

/** Supplies app icons to composables. */
class IconLoader(val peek: (String) -> Bitmap?, val load: suspend (String) -> Bitmap?)

/** Phase 1 features: dock, apps, input, media/controls, deck, files. */
@OptIn(ExperimentalCoroutinesApi::class)
class RemoteViewModel(app: Application) : AndroidViewModel(app) {
    private val controller = (app as HelmApp).controller
    val settings = controller.settings.settings
    val pendingShare: StateFlow<List<SharedItem>> = controller.pendingShare

    private val _ui = MutableStateFlow(RemoteUi())
    val ui: StateFlow<RemoteUi> = _ui.asStateFlow()

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages = _messages.asSharedFlow()

    /** `input.textFocus` from the PC: true = a text field got focus. */
    private val _textFocus = MutableSharedFlow<Boolean>(extraBufferCapacity = 4)
    val textFocus = _textFocus.asSharedFlow()

    private val conn: HelmConnection? get() = controller.session.value?.connection?.takeIf { it.state.value is ConnectionState.Ready }
    private val api: HelmApi? get() = conn?.let { HelmApi(it) }
    private val hostId: String get() = controller.session.value?.hostId ?: ""

    val icons = IconLoader(
        peek = { appId -> controller.icons.peek(hostId, appId) },
        load = { appId -> api?.let { controller.icons.get(it, hostId, appId) } },
    )

    /** G5 uploaded icons, keyed by `c:<hash>`. */
    val customIcons = IconLoader(
        peek = { icon -> controller.icons.peekCustom(hostId, icon) },
        load = { icon -> api?.takeIf { it.conn.supportsG5 }?.let { controller.icons.getCustom(it, hostId, icon) } },
    )
    private var usageJob: Job? = null

    private val motion = MotionBatcher()
    private val scroll = ScrollBatcher()
    private val gyro = com.niranjan.helm.core.GyroMouse()
    private var dragLocked = false
    private val volumeTarget = MutableStateFlow<Float?>(null)
    private val brightnessTarget = MutableStateFlow<Pair<String, Int>?>(null)
    private var volumeDragging = false
    private var windowsJob: Job? = null

    init {
        viewModelScope.launch {
            controller.session.map { it?.connection }.distinctUntilChanged().collect {
                _ui.value = RemoteUi()
                dragLocked = false
            }
        }
        viewModelScope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.filterIsInstance<ConnectionState.Ready>()?.map { s.connection } ?: flowOf() }
                .collect { c -> onReady(c) }
        }
        viewModelScope.launch {
            volumeTarget.filterNotNull().collect { level ->
                act("Volume") { it.setAudio(level = level) }
                delay(40)
            }
        }
        viewModelScope.launch {
            brightnessTarget.filterNotNull().collect { (id, level) ->
                act("Brightness") { it.setBrightness(id, level) }
                delay(60)
            }
        }
    }

    private fun kotlinx.serialization.json.JsonElement?.strOrNull(): String? =
        (this as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content

    private fun message(text: String) {
        _messages.tryEmit(text)
    }

    /** Runs a call, reporting failures as a message. Returns null on failure. */
    private suspend fun <T> act(what: String, block: suspend (HelmApi) -> T): T? {
        val a = api ?: run {
            message("Not connected.")
            return null
        }
        return try {
            block(a)
        } catch (e: HelmException) {
            message("$what failed: ${friendly(e)}")
            null
        }
    }

    private fun friendly(e: HelmException): String = when (e.code) {
        "forbidden" -> "this phone's access level does not allow it."
        "unknown_method" -> "the PC agent does not support this yet."
        else -> e.message ?: e.code
    }

    /** Quiet load: failures are ignored (e.g. a method the agent lacks). */
    private suspend fun <T> quiet(block: suspend (HelmApi) -> T): T? = try {
        api?.let { block(it) }
    } catch (_: HelmException) {
        null
    }

    private suspend fun onReady(c: HelmConnection) = coroutineScope {
        val p1 = c.supportsPhase1
        _ui.update { it.copy(phase1 = p1, phase3 = c.supportsPhase3, phase4 = c.supportsPhase4, phase5 = c.supportsPhase5, g5 = c.supportsG5) }
        launch { c.events.collect { handleEvent(it) } }
        if (!p1) return@coroutineScope
        launch { refreshDock() }
        launch { refreshWindows() }
        launch { refreshMedia(true) }
        launch { quiet { it.audio() }?.let { a -> _ui.update { it.copy(audio = a) } } }
        launch { refreshDisplays() }
        launch { quiet { it.powerStatus() }.let { p -> _ui.update { it.copy(power = p) } } }
        launch { refreshDeck() }
        if (c.supportsG5) launch { refreshUsage() }
        launch { quiet { it.monitors() }?.let { m -> _ui.update { it.copy(monitors = m) } } }
    }

    private fun handleEvent(ev: HelmEvent) {
        when (ev.name) {
            "media.changed" -> {
                val st = MediaState.fromHolder(ev.data)
                val old = _ui.value.media
                _ui.update { it.copy(media = st, mediaAtMs = System.currentTimeMillis()) }
                val trackChanged = st?.title != old?.title || st?.artist != old?.artist || st?.hasArtwork != old?.hasArtwork
                if (trackChanged) viewModelScope.launch { loadArtwork(st) }
            }
            "audio.changed" -> if (!volumeDragging) AudioState.from(ev.data)?.let { a -> _ui.update { it.copy(audio = a) } }
            "windows.changed" -> {
                windowsJob?.cancel()
                windowsJob = viewModelScope.launch {
                    delay(250)
                    refreshWindows()
                }
            }
            "input.textFocus" -> _textFocus.tryEmit(ev.data["focused"]?.toString() == "true")
            "dock.changed" -> viewModelScope.launch { refreshDock() }
            "deck.changed" -> viewModelScope.launch { refreshDeck() }
            "deck.state" -> {
                val id = ev.data["id"].strOrNull() ?: return
                val on = (ev.data["on"] as? kotlinx.serialization.json.JsonPrimitive)?.content == "true"
                _ui.update { it.copy(deckOn = it.deckOn + (id to on)) }
            }
            "deck.widget" -> {
                val id = ev.data["id"].strOrNull() ?: return
                val text = ev.data["text"].strOrNull() ?: ""
                _ui.update { it.copy(deckText = it.deckText + (id to text)) }
            }
            "profile.active" -> {
                val id = ev.data["id"].strOrNull()
                if (id != null && _ui.value.phase3) viewModelScope.launch { followProfile(id) }
            }
            "power.scheduled" -> _ui.update { it.copy(power = PowerSchedule.fromHolder(ev.data)) }
            "clipboard.changed" -> if (_ui.value.clipboardWatch) {
                val text = (ev.data["text"] as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content
                if (text != null) setPhoneClipboard(text, announce = false)
            }
            "input.error" -> {
                val code = ev.data["code"]?.toString()?.trim('"')
                message(
                    if (code == "blocked_by_elevation") "The PC ignored input: an administrator window has focus."
                    else "Input error on the PC: $code",
                )
            }
        }
    }

    // ---- Dock ---------------------------------------------------------------------------

    suspend fun refreshDock() {
        quiet { it.dock2() }?.let { d -> _ui.update { it.copy(dock = d) } }
    }

    private fun saveDock(pages: List<List<DockTile>>) {
        val before = _ui.value.dock
        _ui.update { it.copy(dock = pages) }
        viewModelScope.launch {
            if (act("Saving dock") { it.setDock(pages) } == null) _ui.update { it.copy(dock = before) }
        }
    }

    fun addAppToDock(app: AppEntry) {
        if (DockFolders.leaves(_ui.value.dock).any { it.kind == DockTile.KIND_APP && it.appId == app.appId }) {
            message("${app.name} is already in the dock.")
            return
        }
        val tile = DockTile(newId(), DockTile.KIND_APP, appId = app.appId, label = app.name)
        saveDock(DockCodec.append(_ui.value.dock, tile, settings.value.dockPageSize))
        message("Added ${app.name} to the dock.")
    }

    fun addLinkToDock(target: String, label: String?) {
        val tile = DockTile(newId(), DockTile.KIND_OPEN, target = target.trim(), label = label?.takeIf { it.isNotBlank() })
        saveDock(DockCodec.append(_ui.value.dock, tile, settings.value.dockPageSize))
    }

    fun removeTile(id: String) = saveDock(DockFolders.remove(_ui.value.dock, id))

    fun moveTile(id: String, delta: Int) = saveDock(DockCodec.move(_ui.value.dock, id, delta))

    fun moveTileToPage(id: String, page: Int) = saveDock(DockCodec.moveToPage(_ui.value.dock, id, page))

    fun renameTile(id: String, label: String) = saveDock(
        DockFolders.update(_ui.value.dock, id) { it.copy(label = label.ifBlank { null }) },
    )

    /** G5: icon reference for a tile (null = automatic). */
    fun setTileIcon(id: String, icon: String?) = saveDock(DockFolders.update(_ui.value.dock, id) { it.copy(icon = icon) })

    /** G5: drag [srcId] onto [dstId] to make (or add to) a folder. */
    fun dropTile(srcId: String, dstId: String) {
        if (!_ui.value.g5) {
            message("Folders need a newer Helm agent on the PC.")
            return
        }
        val after = DockFolders.drop(_ui.value.dock, srcId, dstId, newId())
        if (after != _ui.value.dock) saveDock(after)
    }

    /** G5: take a tile out of its folder (it goes right after the folder). */
    fun takeOutOfFolder(childId: String) = saveDock(DockFolders.takeOut(_ui.value.dock, childId))

    fun ungroupFolder(folderId: String) = saveDock(DockFolders.ungroup(_ui.value.dock, folderId))

    // ---- Usage (G5 smart order) ---------------------------------------------------------

    suspend fun refreshUsage() {
        if (!_ui.value.g5) return
        quiet { it.appUsage() }?.let { u -> _ui.update { it.copy(usage = SmartOrder.scores(u)) } }
    }

    /** Re-read usage a moment after a launch (the PC counts it). */
    private fun usageSoon() {
        if (!_ui.value.g5) return
        usageJob?.cancel()
        usageJob = viewModelScope.launch {
            delay(1500)
            refreshUsage()
        }
    }

    /** `profile.active`: show the profile's deck page (its `deck_id` names a page). */
    private suspend fun followProfile(profileId: String) {
        val p = quiet { it.profiles() }?.firstOrNull { it.id == profileId } ?: return
        val page = DeckPages.forProfile(_ui.value.deckPages, p.deckId) ?: return
        _ui.update { it.copy(deckPageWanted = page.id) }
    }

    fun deckPageShown() = _ui.update { it.copy(deckPageWanted = null) }

    /** Tap: focus the app's window if it is running, otherwise launch it (or open the target). */
    fun tapTile(tile: DockTile) {
        viewModelScope.launch {
            when (tile.kind) {
                DockTile.KIND_APP -> {
                    val appId = tile.appId ?: return@launch
                    val win = runningWindow(appId)
                    if (win != null) act("Focus") { it.focus(win.hwnd) } else act("Launch") { it.launch(appId) }
                    usageSoon()
                }
                DockTile.KIND_OPEN -> tile.target?.let { t -> act("Open") { it.open(t) } }
                else -> message("Unknown tile type ${tile.kind}.")
            }
        }
    }

    private fun runningWindow(appId: String): WindowEntry? =
        _ui.value.windows.filter { it.appId.equals(appId, ignoreCase = true) }
            .sortedByDescending { it.isForeground }
            .firstOrNull()

    fun isRunning(appId: String?): Boolean = appId != null && _ui.value.windows.any { it.appId.equals(appId, ignoreCase = true) }

    // ---- Apps & windows -----------------------------------------------------------------

    fun loadApps(force: Boolean = false) {
        if (_ui.value.appsLoading || (_ui.value.apps != null && !force)) return
        _ui.update { it.copy(appsLoading = true) }
        viewModelScope.launch {
            val apps = act("Loading apps") { it.apps() }
            _ui.update { it.copy(apps = apps?.sortedBy { a -> a.name.lowercase() } ?: it.apps, appsLoading = false) }
        }
    }

    fun launchApp(app: AppEntry) {
        viewModelScope.launch {
            val win = runningWindow(app.appId)
            if (win != null) act("Focus") { it.focus(win.hwnd) } else act("Launch") { it.launch(app.appId) }
            usageSoon()
        }
    }

    suspend fun refreshWindows() {
        quiet { it.windows() }?.let { w -> _ui.update { it.copy(windows = w) } }
    }

    fun windowOp(win: WindowEntry, op: String) {
        viewModelScope.launch {
            if (op == "focus") act("Focus") { it.focus(win.hwnd) } else act("Window $op") { it.windowOp(op, win.hwnd) }
            refreshWindows()
        }
    }

    fun openOnPc(target: String) {
        val t = target.trim()
        if (t.isEmpty()) return
        viewModelScope.launch { if (act("Open") { it.open(t) } != null) message("Opened on the PC.") }
    }

    // ---- Input --------------------------------------------------------------------------

    fun send(frame: InputFrame) {
        conn?.sendInput(frame)
    }

    fun sendAll(frames: List<InputFrame>) {
        val c = conn ?: return
        for (f in frames) c.sendInput(f)
    }

    fun touchMove(dx: Float, dy: Float) = motion.add(dx, dy)

    fun touchScroll(dx: Float, dy: Float) = scroll.add(dx, dy)

    /** Called about 60 times a second while the touchpad is visible. */
    fun flushInput() {
        val c = conn ?: return
        motion.drain()?.let { c.sendInput(it) }
        scroll.drain()?.let { c.sendInput(it) }
        gyro.drain()?.let { c.sendInput(it) }
    }

    /** Gyro air mouse (§12 `gy`, p4 only): angular rates in rad/s. */
    fun gyroSample(rateX: Float, rateZ: Float, dtS: Float, speed: Float) {
        if (_ui.value.phase4) gyro.add(rateX, rateZ, dtS, speed)
    }

    fun gyroReset() = gyro.reset()

    fun click(button: MouseButton) {
        flushInput()
        if (dragLocked && button == MouseButton.LEFT) {
            setDragLock(false)
            return
        }
        sendAll(InputFrame.click(button))
    }

    fun pinch(start: Boolean) {
        flushInput()
        send(if (start) InputFrame.KeyDown(Vk.CONTROL) else InputFrame.KeyUp(Vk.CONTROL))
    }

    fun setDragLock(on: Boolean): Boolean {
        if (on == dragLocked) return dragLocked
        flushInput()
        send(InputFrame.Button(MouseButton.LEFT, on))
        dragLocked = on
        return dragLocked
    }

    val isDragLocked: Boolean get() = dragLocked

    /** Saves to this PC's own speed when it has one (per-PC settings), else the global one. */
    fun setTouchpadSpeed(v: Float) {
        val id = controller.session.value?.hostId
        if (id != null && controller.prefsFor(id).touchpadSpeed != null) controller.updatePcPrefs(id) { it.copy(touchpadSpeed = v) }
        else controller.settings.update { it.copy(touchpadSpeed = v) }
    }

    // ---- Media, audio, brightness, power ------------------------------------------------

    private suspend fun refreshMedia(withArtwork: Boolean) {
        val st = quiet { it.mediaState() }
        _ui.update { it.copy(media = st, mediaAtMs = System.currentTimeMillis()) }
        if (withArtwork) loadArtwork(st)
    }

    private suspend fun loadArtwork(st: MediaState?) {
        if (st == null || !st.hasArtwork) {
            _ui.update { it.copy(artwork = null) }
            return
        }
        val png = quiet { it.mediaArtwork() }
        val bmp = png?.let { withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(it, 0, it.size) } }
        _ui.update { it.copy(artwork = bmp) }
    }

    fun media(action: String) {
        viewModelScope.launch { act("Media") { it.mediaControl(action) } }
    }

    fun seek(ms: Long) {
        viewModelScope.launch { act("Seek") { it.mediaSeek(ms) } }
    }

    fun setVolume(level: Float, dragging: Boolean) {
        volumeDragging = dragging
        _ui.update { it.copy(audio = (it.audio ?: AudioState(level, false)).copy(level = level)) }
        volumeTarget.value = level
    }

    fun toggleMute() {
        val a = _ui.value.audio ?: return
        _ui.update { it.copy(audio = a.copy(muted = !a.muted)) }
        viewModelScope.launch { act("Mute") { it.setAudio(muted = !a.muted) } }
    }

    private suspend fun refreshDisplays() {
        quiet { it.brightness() }?.let { d -> _ui.update { it.copy(displays = d) } }
    }

    fun setBrightness(id: String, level: Int) {
        _ui.update { u -> u.copy(displays = u.displays.map { if (it.id == id) it.copy(level = level) else it }) }
        brightnessTarget.value = id to level
    }

    fun power(action: String, delayS: Int?) {
        viewModelScope.launch {
            val r = act("Power") { listOf(it.powerAction(action, delayS)) } ?: return@launch
            _ui.update { it.copy(power = r[0]) }
            if (r[0] == null) message("Sent ${action.replace('_', ' ')} to the PC.")
        }
    }

    fun cancelPower() {
        viewModelScope.launch {
            val r = act("Cancel") { listOf(it.powerCancel()) } ?: return@launch
            _ui.update { it.copy(power = r[0]) }
            if (r[0] == null) message("Cancelled.")
        }
    }

    // ---- Deck ---------------------------------------------------------------------------

    suspend fun refreshDeck() {
        quiet { it.deck2() }?.let { l -> _ui.update { it.copy(deck = l.buttons, deckPages = l.pages) } }
    }

    private fun saveDeck(layout: DeckLayout) {
        val before = _ui.value.deckLayout
        val pages = DeckPages.normalize(layout.pages, layout.buttons)
        _ui.update { it.copy(deck = layout.buttons, deckPages = pages) }
        viewModelScope.launch {
            if (act("Saving deck") { it.setDeck2(DeckLayout(layout.buttons, pages)) } == null) {
                _ui.update { it.copy(deck = before.buttons, deckPages = before.pages) }
            }
        }
    }

    /** Adds or updates a button; a new one goes to [pageId] (default: the first page). */
    fun saveDeckButton(b: DeckButton, pageId: String? = null) =
        saveDeck(DeckPages.upsert(_ui.value.deckLayout, b, pageId ?: _ui.value.deckPages.first().id))

    fun deleteDeckButton(id: String) = saveDeck(DeckPages.delete(_ui.value.deckLayout, id))

    fun moveDeckButton(id: String, delta: Int) = saveDeck(DeckPages.shift(_ui.value.deckLayout, id, delta))

    fun moveDeckButtonToPage(id: String, pageId: String) = saveDeck(DeckPages.moveTo(_ui.value.deckLayout, id, pageId))

    fun addDeckPage(name: String): String? {
        if (!_ui.value.g5) {
            message("Deck pages need a newer Helm agent on the PC.")
            return null
        }
        val id = newId()
        saveDeck(DeckPages.addPage(_ui.value.deckLayout, id, name.trim().ifEmpty { "Page ${_ui.value.deckPages.size + 1}" }))
        return id
    }

    fun renameDeckPage(id: String, name: String) = saveDeck(DeckPages.renamePage(_ui.value.deckLayout, id, name.trim()))

    fun deleteDeckPage(id: String) = saveDeck(DeckPages.deletePage(_ui.value.deckLayout, id))

    fun runDeck(b: DeckButton) {
        viewModelScope.launch {
            act(b.label.ifBlank { "Button" }) { it.runDeck(b.id) }
            if (b.action is com.niranjan.helm.core.DeckAction.Launch) usageSoon()
        }
    }

    /**
     * G5: uploads a picture from the phone as a custom icon (center-cropped to
     * [CustomIcon.SIDE_PX] px, PNG). Returns the icon reference, or null on failure.
     */
    suspend fun uploadIcon(uri: Uri): String? {
        val ctx = getApplication<Application>()
        val png = withContext(Dispatchers.Default) {
            runCatching {
                val src = ctx.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it) } ?: return@runCatching null
                val side = minOf(src.width, src.height)
                val square = Bitmap.createBitmap(src, (src.width - side) / 2, (src.height - side) / 2, side, side)
                val scaled = Bitmap.createScaledBitmap(square, CustomIcon.SIDE_PX, CustomIcon.SIDE_PX, true)
                java.io.ByteArrayOutputStream().also { scaled.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
            }.getOrNull()
        }
        if (png == null || png.size > CustomIcon.MAX_BYTES) {
            message("Could not read that picture.")
            return null
        }
        val icon = act("Uploading icon") { it.putIcon(png) } ?: return null
        controller.icons.putCustom(hostId, icon, png)
        return icon
    }

    fun newId(): String = UUID.randomUUID().toString().take(8)

    // ---- Files, clipboard, screenshot ---------------------------------------------------

    fun uploadUris(uris: List<Uri>) {
        if (uris.isEmpty()) return
        val c = conn ?: return message("Not connected.")
        viewModelScope.launch {
            var done = 0
            for ((i, uri) in uris.withIndex()) {
                val prefix = if (uris.size > 1) "(${i + 1}/${uris.size}) " else ""
                try {
                    _ui.update { it.copy(transfer = TransferUi("${prefix}Preparing…", null)) }
                    val f = withContext(Dispatchers.IO) { controller.files.describe(uri) }
                    _ui.update { it.copy(transfer = TransferUi("$prefix${f.name}", 0f)) }
                    c.uploadFile(f.name, f.size, f.sha256, { controller.files.open(uri) }) { sent ->
                        val p = if (f.size == 0L) 1f else sent.toFloat() / f.size
                        _ui.update { it.copy(transfer = TransferUi("$prefix${f.name}", p)) }
                    }
                    done++
                } catch (e: HelmException) {
                    message("Upload failed: ${friendly(e)}")
                } catch (e: Exception) {
                    message("Could not read the file: ${e.message}")
                }
            }
            _ui.update { it.copy(transfer = null) }
            if (done > 0) message(if (done == 1) "Sent to the PC's Downloads\\Helm." else "Sent $done files to the PC's Downloads\\Helm.")
        }
    }

    /** Sends everything shared into Helm: files are uploaded, URLs opened, text copied. */
    fun sendPendingShare() {
        val items = controller.pendingShare.value
        controller.pendingShare.value = emptyList()
        val files = items.filterIsInstance<SharedItem.File>().map { it.uri }
        val texts = items.filterIsInstance<SharedItem.Text>()
        viewModelScope.launch {
            for (t in texts) {
                if (t.isUrl) {
                    if (act("Open") { it.open(t.text.trim()) } != null) message("Opened the link on the PC.")
                } else if (act("Clipboard") { it.clipboardSetText(t.text) } != null) {
                    message("Text copied to the PC's clipboard.")
                }
            }
        }
        uploadUris(files)
    }

    fun discardPendingShare() {
        controller.pendingShare.value = emptyList()
    }

    fun pushClipboard() {
        val cm = getApplication<Application>().getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val text = cm.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(getApplication())?.toString()
        if (text.isNullOrEmpty()) return message("The phone clipboard has no text.")
        viewModelScope.launch { if (act("Clipboard") { it.clipboardSetText(text) } != null) message("Sent to the PC clipboard.") }
    }

    fun pullClipboard() {
        viewModelScope.launch {
            val clip = act("Clipboard") { it.clipboardGet() } ?: return@launch
            when {
                clip.text != null -> setPhoneClipboard(clip.text, announce = true)
                clip.png != null -> {
                    val bmp = withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(clip.png, 0, clip.png.size) }
                    _ui.update { it.copy(pcClipImage = bmp, screenshotPng = clip.png, screenshot = bmp) }
                    message("The PC clipboard holds an image; it is shown under Screenshot.")
                }
                else -> message("The PC clipboard is empty.")
            }
        }
    }

    private fun setPhoneClipboard(text: String, announce: Boolean) {
        val cm = getApplication<Application>().getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        cm.setPrimaryClip(ClipData.newPlainText("From PC", text))
        if (announce) message("Copied the PC clipboard to this phone.")
    }

    fun setClipboardWatch(on: Boolean) {
        viewModelScope.launch {
            if (act("Clipboard sync") { it.clipboardWatch(on) } != null) _ui.update { it.copy(clipboardWatch = on) }
        }
    }

    fun capture(monitor: MonitorInfo?) {
        _ui.update { it.copy(capturing = true) }
        viewModelScope.launch {
            val png = act("Screenshot") { it.capture(monitor?.id) }
            val bmp = png?.let { withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(it, 0, it.size) } }
            _ui.update { it.copy(capturing = false, screenshot = bmp ?: it.screenshot, screenshotPng = png ?: it.screenshotPng) }
            if (png != null && bmp == null) message("The PC sent an image this phone cannot read.")
        }
    }

    fun saveScreenshot() {
        val png = _ui.value.screenshotPng ?: return
        viewModelScope.launch {
            try {
                val name = "Helm_" + SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date()) + ".png"
                val where = withContext(Dispatchers.IO) { controller.files.savePngToGallery(png, name) }
                message("Saved to $where.")
            } catch (e: Exception) {
                message("Could not save: ${e.message}")
            }
        }
    }

    fun download(path: String) {
        val p = path.trim().trim('"')
        if (p.isEmpty()) return
        val c = conn ?: return message("Not connected.")
        viewModelScope.launch {
            val name = p.substringAfterLast('\\').substringAfterLast('/').ifBlank { "download" }
            var sink: com.niranjan.helm.device.PhoneFiles.Sink? = null
            try {
                _ui.update { it.copy(transfer = TransferUi("Receiving $name", null)) }
                sink = withContext(Dispatchers.IO) { controller.files.createDownload(name) }
                val s = sink
                c.downloadFile(p, s.out) { got, total ->
                    _ui.update { it.copy(transfer = TransferUi("Receiving $name", if (total > 0) got.toFloat() / total else null)) }
                }
                withContext(Dispatchers.IO) {
                    s.out.close()
                    s.commit()
                }
                message("Saved to ${s.where}.")
            } catch (e: HelmException) {
                withContext(Dispatchers.IO) { runCatching { sink?.out?.close(); sink?.abort?.invoke() } }
                message("Download failed: ${friendly(e)}")
            } catch (e: Exception) {
                withContext(Dispatchers.IO) { runCatching { sink?.out?.close(); sink?.abort?.invoke() } }
                message("Could not save the file: ${e.message}")
            } finally {
                _ui.update { it.copy(transfer = null) }
            }
        }
    }

    override fun onCleared() {
        // Never leave the PC's left button held.
        if (dragLocked) conn?.sendInput(InputFrame.Button(MouseButton.LEFT, false))
        super.onCleared()
    }
}
