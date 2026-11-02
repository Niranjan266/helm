package com.niranjan.helm.core

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * G5 (Docs/protocol/dock-deck-2.md): dock folders, usage-based "smart order", deck pages and
 * button sizes, icons (real app icons, built-in stroke icons, custom uploads).
 */
object G5 {
    const val FEATURE = "g5"
}

// ---------------------------------------------------------------- dock folders

/** Folder helpers over dock pages. Folders live at the top level and hold app/open tiles only. */
object DockFolders {
    const val MAX_CHILDREN = 32

    /** Every app/open tile, folders opened up. */
    fun leaves(pages: List<List<DockTile>>): List<DockTile> =
        pages.flatten().flatMap { if (it.kind == DockTile.KIND_FOLDER) it.children else listOf(it) }

    fun find(pages: List<List<DockTile>>, id: String): DockTile? =
        pages.flatten().firstNotNullOfOrNull { t -> if (t.id == id) t else t.children.firstOrNull { it.id == id } }

    /** Applies [change] to the tile with [id] wherever it is (top level or inside a folder). */
    fun update(pages: List<List<DockTile>>, id: String, change: (DockTile) -> DockTile): List<List<DockTile>> =
        pages.map { p ->
            p.map { t ->
                when {
                    t.id == id -> change(t)
                    t.kind == DockTile.KIND_FOLDER && t.children.any { it.id == id } ->
                        t.copy(children = t.children.map { c -> if (c.id == id) change(c) else c })
                    else -> t
                }
            }
        }

    /** Removes a tile anywhere; a folder left with one tile becomes that tile, an empty one goes. */
    fun remove(pages: List<List<DockTile>>, id: String): List<List<DockTile>> =
        pages.map { p ->
            p.mapNotNull { t ->
                when {
                    t.id == id -> null
                    t.kind == DockTile.KIND_FOLDER && t.children.any { it.id == id } -> settle(t.copy(children = t.children.filterNot { it.id == id }))
                    else -> t
                }
            }
        }.filter { it.isNotEmpty() }

    private fun settle(folder: DockTile): DockTile? = when (folder.children.size) {
        0 -> null
        1 -> folder.children[0]
        else -> folder
    }

    /**
     * Drag [srcId] onto [dstId]: dropping on a folder adds to it; dropping on a tile makes a new
     * folder ([folderId], [name]) in the target's place. Folders can be merged into folders but
     * never nested. Unchanged when either id is missing, they are equal, or the folder is full.
     */
    fun drop(pages: List<List<DockTile>>, srcId: String, dstId: String, folderId: String, name: String = "Folder"): List<List<DockTile>> {
        if (srcId == dstId) return pages
        val top = pages.flatten()
        val src = top.firstOrNull { it.id == srcId } ?: find(pages, srcId) ?: return pages
        val dst = top.firstOrNull { it.id == dstId } ?: return pages
        val moving = if (src.kind == DockTile.KIND_FOLDER) src.children else listOf(src)
        val replacement = when {
            dst.kind == DockTile.KIND_FOLDER -> {
                if (dst.children.size + moving.size > MAX_CHILDREN) return pages
                dst.copy(children = dst.children + moving)
            }
            src.kind == DockTile.KIND_FOLDER -> {
                if (src.children.size + 1 > MAX_CHILDREN) return pages
                src.copy(children = listOf(dst) + src.children)
            }
            else -> DockTile(folderId, DockTile.KIND_FOLDER, label = name, children = listOf(dst, src))
        }
        val placed = pages.map { p -> p.map { if (it.id == dstId) replacement else it } }
        // Remove the source from where it was (it may already be inside the replacement).
        return placed.map { p ->
            p.mapNotNull { t ->
                when {
                    t === replacement -> t
                    t.id == srcId -> null
                    t.kind == DockTile.KIND_FOLDER && t.children.any { it.id == srcId } -> settle(t.copy(children = t.children.filterNot { it.id == srcId }))
                    else -> t
                }
            }
        }.filter { it.isNotEmpty() }
    }

    /** Replaces a folder by its tiles, in place. */
    fun ungroup(pages: List<List<DockTile>>, folderId: String): List<List<DockTile>> =
        pages.map { p -> p.flatMap { if (it.id == folderId && it.kind == DockTile.KIND_FOLDER) it.children else listOf(it) } }

    /** Takes [childId] out of its folder and puts it right after the folder on the same page. */
    fun takeOut(pages: List<List<DockTile>>, childId: String): List<List<DockTile>> =
        pages.map { p ->
            p.flatMap { t ->
                val child = if (t.kind == DockTile.KIND_FOLDER) t.children.firstOrNull { it.id == childId } else null
                if (child == null) listOf(t)
                else listOfNotNull(settle(t.copy(children = t.children.filterNot { it.id == childId })), child)
            }
        }
}

// ---------------------------------------------------------------- usage & smart order

/** `apps.usage` entry. */
data class AppUsage(val appId: String, val launches: Long, val focus: Long, val lastUsedMs: Long, val score: Double) {
    companion object {
        fun listFrom(result: JsonObject): List<AppUsage> = result.objList("apps").mapNotNull { o ->
            val id = o.optStr("app_id") ?: return@mapNotNull null
            AppUsage(id, o.optLong("launches") ?: 0, o.optLong("focus") ?: 0, o.optLong("last_used_ms") ?: 0, o.optDouble("score") ?: 0.0)
        }
    }
}

/** Sorting by how much apps are used (the PC ranks; the phone only orders by its scores). */
object SmartOrder {
    fun scores(usage: List<AppUsage>): Map<String, Double> =
        usage.associate { it.appId.lowercase() to it.score }

    private fun scoreOf(appId: String?, scores: Map<String, Double>): Double =
        appId?.let { scores[it.lowercase()] } ?: 0.0

    /** A folder ranks by its most used tile; links have no usage. */
    fun tileScore(t: DockTile, scores: Map<String, Double>): Double = when (t.kind) {
        DockTile.KIND_FOLDER -> t.children.maxOfOrNull { tileScore(it, scores) } ?: 0.0
        DockTile.KIND_APP -> scoreOf(t.appId, scores)
        else -> 0.0
    }

    /** Most used first; equal scores keep their saved order (stable sort). */
    fun sortTiles(tiles: List<DockTile>, scores: Map<String, Double>): List<DockTile> =
        tiles.sortedByDescending { tileScore(it, scores) }

    /** Most used first, then by name. */
    fun sortApps(apps: List<AppEntry>, scores: Map<String, Double>): List<AppEntry> =
        apps.sortedWith(compareByDescending<AppEntry> { scoreOf(it.appId, scores) }.thenBy { it.name.lowercase() })
}

// ---------------------------------------------------------------- deck pages

data class DeckPage(val id: String, val name: String, val buttons: List<String>)

/** `deck.get{v:2}` result: every button plus the pages that order them. */
data class DeckLayout(val buttons: List<DeckButton>, val pages: List<DeckPage>) {
    fun buttonsOn(pageId: String): List<DeckButton> {
        val byId = buttons.associateBy { it.id }
        return pages.firstOrNull { it.id == pageId }?.buttons.orEmpty().mapNotNull { byId[it] }
    }

    fun pageOf(buttonId: String): DeckPage? = pages.firstOrNull { buttonId in it.buttons }
}

object DeckPages {
    const val DEFAULT_ID = "main"
    const val MAX_PAGES = 32

    /** Same rules as the agent: ids kept once and only if the button exists, the rest go to page 1. */
    fun normalize(pages: List<DeckPage>, buttons: List<DeckButton>): List<DeckPage> {
        val base = pages.ifEmpty { listOf(DeckPage(DEFAULT_ID, "Main", emptyList())) }
        val known = buttons.map { it.id }.toSet()
        val seen = mutableSetOf<String>()
        val cleaned = base.map { p -> p.copy(buttons = p.buttons.filter { it in known && seen.add(it) }) }
        val rest = buttons.map { it.id }.filterNot { it in seen }
        return cleaned.mapIndexed { i, p -> if (i == 0) p.copy(buttons = p.buttons + rest) else p }
    }

    fun decode(result: JsonObject): DeckLayout {
        val buttons = DeckCodec.decode(result)
        val pages = result.objList("pages").mapNotNull { o ->
            val id = o.optStr("id") ?: return@mapNotNull null
            val ids = (o["buttons"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.contentOrNull }
            DeckPage(id, o.optStr("name") ?: "", ids)
        }
        return DeckLayout(buttons, normalize(pages, buttons))
    }

    /** Params for `deck.set` with pages (G5). Phase 1 agents ignore `pages` and `size`. */
    fun encode(layout: DeckLayout): JsonObject {
        val base = DeckCodec.encode(layout.buttons)
        return buildJsonObject {
            base.forEach { (k, v) -> put(k, v) }
            put("pages", buildJsonArray {
                for (p in normalize(layout.pages, layout.buttons)) addJsonObject {
                    put("id", p.id)
                    put("name", p.name)
                    put("buttons", buildJsonArray { p.buttons.forEach { add(JsonPrimitive(it)) } })
                }
            })
        }
    }

    fun addPage(l: DeckLayout, id: String, name: String): DeckLayout =
        if (l.pages.size >= MAX_PAGES) l else l.copy(pages = l.pages + DeckPage(id, name, emptyList()))

    fun renamePage(l: DeckLayout, id: String, name: String): DeckLayout =
        l.copy(pages = l.pages.map { if (it.id == id) it.copy(name = name) else it })

    /** Deletes a page; its buttons move to the first remaining page. The last page stays. */
    fun deletePage(l: DeckLayout, id: String): DeckLayout {
        if (l.pages.size <= 1) return l
        val gone = l.pages.firstOrNull { it.id == id } ?: return l
        val left = l.pages.filterNot { it.id == id }
        return l.copy(pages = left.mapIndexed { i, p -> if (i == 0) p.copy(buttons = p.buttons + gone.buttons) else p })
    }

    /** Puts [buttonId] at the end of [pageId] (removed from any other page). */
    fun moveTo(l: DeckLayout, buttonId: String, pageId: String): DeckLayout {
        if (l.pages.none { it.id == pageId }) return l
        return l.copy(pages = l.pages.map { p ->
            val without = p.buttons - buttonId
            if (p.id == pageId) p.copy(buttons = without + buttonId) else p.copy(buttons = without)
        })
    }

    /** Moves [buttonId] by [delta] within its page. */
    fun shift(l: DeckLayout, buttonId: String, delta: Int): DeckLayout = l.copy(pages = l.pages.map { p ->
        val i = p.buttons.indexOf(buttonId)
        val j = i + delta
        if (i < 0 || j !in p.buttons.indices) p
        else p.copy(buttons = p.buttons.toMutableList().apply { add(j, removeAt(i)) })
    })

    /** Adds or replaces a button; a new one goes to the end of [pageId]. */
    fun upsert(l: DeckLayout, b: DeckButton, pageId: String): DeckLayout {
        if (l.buttons.any { it.id == b.id }) return l.copy(buttons = l.buttons.map { if (it.id == b.id) b else it })
        val pages = normalize(l.pages, l.buttons)
        val target = pages.firstOrNull { it.id == pageId }?.id ?: pages[0].id
        return l.copy(buttons = l.buttons + b, pages = pages.map { if (it.id == target) it.copy(buttons = it.buttons + b.id) else it })
    }

    fun delete(l: DeckLayout, buttonId: String): DeckLayout =
        l.copy(buttons = l.buttons.filterNot { it.id == buttonId }, pages = l.pages.map { it.copy(buttons = it.buttons - buttonId) })

    /** Page for a profile's `deck_id` (page id, or else a page name, case-insensitive). */
    fun forProfile(pages: List<DeckPage>, deckId: String?): DeckPage? {
        val key = deckId?.trim()?.takeIf { it.isNotEmpty() } ?: return null
        return pages.firstOrNull { it.id == key } ?: pages.firstOrNull { it.name.equals(key, ignoreCase = true) }
    }
}

/** Deck button size in grid cells. */
enum class ButtonSize(val wire: String, val cols: Int, val rows: Int, val label: String) {
    ONE("1x1", 1, 1, "1×1"),
    WIDE("2x1", 2, 1, "2×1"),
    BIG("2x2", 2, 2, "2×2");

    companion object {
        fun of(wire: String?): ButtonSize = entries.firstOrNull { it.wire == wire } ?: ONE
    }
}

/** Placement of one button in the deck grid. */
data class GridCell(val row: Int, val col: Int, val cols: Int, val rows: Int)

object DeckGrid {
    /**
     * First-fit packing, in order: each button takes the first free spot (row by row) where it
     * fits. Buttons wider than the grid shrink to its width.
     */
    fun pack(sizes: List<ButtonSize>, columns: Int): List<GridCell> {
        val cols = columns.coerceAtLeast(1)
        val used = mutableListOf<BooleanArray>()
        fun free(r: Int, c: Int): Boolean = r >= used.size || !used[r][c]
        val out = mutableListOf<GridCell>()
        for (s in sizes) {
            val w = s.cols.coerceAtMost(cols)
            val h = s.rows
            var r = 0
            placed@ while (true) {
                for (c in 0..cols - w) {
                    val fits = (r until r + h).all { rr -> (c until c + w).all { cc -> free(rr, cc) } }
                    if (fits) {
                        while (used.size < r + h) used += BooleanArray(cols)
                        for (rr in r until r + h) for (cc in c until c + w) used[rr][cc] = true
                        out += GridCell(r, c, w, h)
                        break@placed
                    }
                }
                r++
            }
        }
        return out
    }

    fun rowCount(cells: List<GridCell>): Int = cells.maxOfOrNull { it.row + it.rows } ?: 0
}

// ---------------------------------------------------------------- icons

/** What a deck button or dock tile shows as its picture. */
sealed interface IconRef {
    /** One of [BUILTIN_ICONS], drawn on the phone. */
    data class Builtin(val name: String) : IconRef
    /** Uploaded image, fetched with `deck.icon.get`. */
    data class Custom(val id: String) : IconRef
    /** The PC app's real icon (`apps.icon`). */
    data class App(val appId: String) : IconRef
    /** Older free-text icon value (shown as text). */
    data class Legacy(val text: String) : IconRef
    data object None : IconRef

    companion object {
        const val BUILTIN_PREFIX = "b:"
        const val CUSTOM_PREFIX = "c:"
        private val CUSTOM = Regex("^c:[0-9a-f]{32}$")

        fun builtin(name: String) = "$BUILTIN_PREFIX$name"

        /** Parses a stored `icon` value; blank = [None]. */
        fun parse(icon: String?): IconRef {
            val s = icon?.trim().orEmpty()
            return when {
                s.isEmpty() -> None
                s.startsWith(BUILTIN_PREFIX) && s.removePrefix(BUILTIN_PREFIX) in BUILTIN_ICONS -> Builtin(s.removePrefix(BUILTIN_PREFIX))
                CUSTOM.matches(s) -> Custom(s)
                else -> Legacy(s)
            }
        }

        /** Picture for an `open` target: an app whose id is the target, else globe / folder / file. */
        fun forTarget(target: String, apps: List<AppEntry>?): IconRef {
            apps?.firstOrNull { it.appId.equals(target, ignoreCase = true) }?.let { return App(it.appId) }
            val t = target.trim().lowercase()
            return when {
                t.startsWith("http://") || t.startsWith("https://") -> Builtin("globe")
                t.startsWith("mailto:") -> Builtin("mail")
                t.endsWith("\\") || t.endsWith("/") || !t.substringAfterLast('\\').contains('.') -> Builtin("folder")
                else -> Builtin("file")
            }
        }

        /** Explicit icon first; otherwise the real app icon for launch/open, else a fitting built-in. */
        fun forButton(b: DeckButton, apps: List<AppEntry>?): IconRef {
            parse(b.icon).takeIf { it != None }?.let { return it }
            return when (val a = b.action) {
                is DeckAction.Launch -> App(a.appId)
                is DeckAction.Open -> forTarget(a.target, apps)
                is DeckAction.Media -> Builtin(
                    when (a.action) {
                        "next" -> "next"
                        "previous" -> "prev"
                        "pause", "stop" -> "pause"
                        else -> "play"
                    },
                )
                is DeckAction.VolumeStep -> Builtin(if (a.step >= 0) "volume_up" else "volume_down")
                is DeckAction.Hotkey -> Builtin("keyboard")
                is DeckAction.Text -> Builtin("text")
                is DeckAction.Macro, is DeckAction.Repeat -> Builtin("bolt")
                is DeckAction.Ha -> Builtin("bulb")
                else -> None
            }
        }

        fun forTile(t: DockTile, apps: List<AppEntry>?): IconRef {
            parse(t.icon).takeIf { it != None }?.let { return it }
            return when (t.kind) {
                DockTile.KIND_APP -> t.appId?.let { App(it) } ?: None
                DockTile.KIND_OPEN -> t.target?.let { forTarget(it, apps) } ?: None
                DockTile.KIND_FOLDER -> Builtin("folder")
                else -> None
            }
        }
    }
}

/** Names of the built-in stroke icons (drawn by the phone; see ui/BuiltinIcons.kt). */
val BUILTIN_ICONS = listOf(
    "play", "pause", "next", "prev", "volume_up", "volume_down", "mute", "mic", "mic_off",
    "power", "lock", "globe", "folder", "file", "terminal", "gear", "star", "heart", "home",
    "mail", "camera", "music", "keyboard", "monitor", "bulb", "code", "chat", "game", "bolt", "text",
)

/** Icon upload limits (the agent refuses larger files). */
object CustomIcon {
    const val SIDE_PX = 128
    const val MAX_BYTES = 256 * 1024
}
