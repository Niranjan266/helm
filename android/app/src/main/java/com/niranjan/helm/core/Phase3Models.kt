package com.niranjan.helm.core

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put

/** `hello.features` names. */
object Features {
    const val P1 = "p1"
    const val P2 = "p2"
    const val P3 = "p3"
    const val P4 = "p4"
    const val P5 = "p5"
}

private fun JsonObject.floats(k: String): List<Float> =
    (this[k] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.doubleOrNull?.toFloat() }

data class DiskStats(val mount: String, val label: String, val used: Long, val total: Long, val readBps: Long, val writeBps: Long) {
    val freePct: Float get() = if (total <= 0) 0f else ((total - used) * 100.0 / total).toFloat()
}

data class NetStats(val name: String, val rxBps: Long, val txBps: Long)

data class GpuStats(val name: String, val loadPct: Float, val vramUsed: Long, val vramTotal: Long)

/** `stats.tick` data: the `helm-win-sysmon` `Stats` struct (§11). */
data class Stats(
    val timestampMs: Long,
    val cpuTotal: Float,
    val cpuPerCore: List<Float>,
    val ramUsed: Long,
    val ramTotal: Long,
    val disks: List<DiskStats>,
    val net: List<NetStats>,
    val gpu: List<GpuStats>,
) {
    val ramPct: Float get() = if (ramTotal <= 0) 0f else (ramUsed * 100.0 / ramTotal).toFloat()
    val netRxBps: Long get() = net.sumOf { it.rxBps }
    val netTxBps: Long get() = net.sumOf { it.txBps }

    companion object {
        fun from(o: JsonObject): Stats = Stats(
            timestampMs = o.optLong("timestamp_ms") ?: 0,
            cpuTotal = (o.optDouble("cpu_total") ?: 0.0).toFloat(),
            cpuPerCore = o.floats("cpu_per_core"),
            ramUsed = o.optLong("ram_used") ?: 0,
            ramTotal = o.optLong("ram_total") ?: 0,
            disks = o.objList("disks").map { d ->
                DiskStats(
                    d.optStr("mount") ?: "", d.optStr("label") ?: "", d.optLong("used") ?: 0, d.optLong("total") ?: 0,
                    d.optLong("read_bps") ?: 0, d.optLong("write_bps") ?: 0,
                )
            },
            net = o.objList("net").map { n -> NetStats(n.optStr("name") ?: "", n.optLong("rx_bps") ?: 0, n.optLong("tx_bps") ?: 0) },
            gpu = o.objList("gpu").map { g ->
                GpuStats(g.optStr("name") ?: "GPU", (g.optDouble("load_pct") ?: 0.0).toFloat(), g.optLong("vram_used") ?: 0, g.optLong("vram_total") ?: 0)
            },
        )
    }
}

/** Fixed-size history of samples for the live charts. */
class StatsHistory(val capacity: Int = 120) {
    private val items = ArrayDeque<Stats>()

    @Synchronized
    fun add(s: Stats): List<Stats> {
        items.addLast(s)
        while (items.size > capacity) items.removeFirst()
        return items.toList()
    }

    @Synchronized
    fun clear() = items.clear()
}

/** `alerts.get` / `alerts.set` rule. */
data class AlertRule(val id: String, val kind: String, val threshold: Double, val forS: Int? = null, val mount: String? = null) {
    companion object {
        const val CPU_ABOVE = "cpu_above"
        const val RAM_ABOVE = "ram_above"
        const val DISK_FREE_BELOW = "disk_free_below"
        val KINDS = listOf(CPU_ABOVE, RAM_ABOVE, DISK_FREE_BELOW)

        fun listFrom(o: JsonObject): List<AlertRule> = o.objList("rules").mapNotNull { r ->
            AlertRule(
                id = r.optStr("id") ?: return@mapNotNull null,
                kind = r.optStr("kind") ?: return@mapNotNull null,
                threshold = r.optDouble("threshold") ?: return@mapNotNull null,
                forS = r.optLong("for_s")?.toInt(),
                mount = r.optStr("mount"),
            )
        }

        fun encode(rules: List<AlertRule>): JsonObject = buildJsonObject {
            put("rules", buildJsonArray {
                for (r in rules) addJsonObject {
                    put("id", r.id)
                    put("kind", r.kind)
                    put("threshold", r.threshold)
                    r.forS?.let { put("for_s", it) }
                    r.mount?.takeIf { it.isNotBlank() }?.let { put("mount", it) }
                }
            })
        }
    }

    fun describe(): String = when (kind) {
        CPU_ABOVE -> "CPU above ${threshold.fmt()}%" + (forS?.takeIf { it > 0 }?.let { " for $it s" } ?: "")
        RAM_ABOVE -> "RAM above ${threshold.fmt()}%"
        DISK_FREE_BELOW -> "Free space on ${mount ?: "any disk"} below ${threshold.fmt()}%"
        else -> "$kind ${threshold.fmt()}"
    }

    private fun Double.fmt() = if (this % 1.0 == 0.0) toLong().toString() else toString()
}

/** `alert` event. */
data class AlertEvent(val id: String, val firing: Boolean, val value: Double, val message: String?) {
    companion object {
        fun from(o: JsonObject): AlertEvent? {
            val id = o.optStr("id") ?: o.optStr("rule_id") ?: return null
            return AlertEvent(id, o.optBool("firing") == true, o.optDouble("value") ?: 0.0, o.optStr("message"))
        }
    }
}

/** `process.list` entry. */
data class ProcessEntry(val pid: Long, val name: String, val exe: String?, val cpuPct: Float, val ramBytes: Long) {
    companion object {
        val PRIORITIES = listOf("idle", "below_normal", "normal", "above_normal", "high")

        fun listFrom(o: JsonObject): List<ProcessEntry> = o.objList("processes").mapNotNull { p ->
            ProcessEntry(
                p.optLong("pid") ?: return@mapNotNull null, p.optStr("name") ?: "?", p.optStr("exe"),
                (p.optDouble("cpu_pct") ?: 0.0).toFloat(), p.optLong("ram_bytes") ?: 0,
            )
        }
    }
}

enum class ProcessSort { CPU, RAM, NAME }

object ProcessSorter {
    fun sort(list: List<ProcessEntry>, by: ProcessSort, filter: String = ""): List<ProcessEntry> {
        val q = filter.trim().lowercase()
        val shown = if (q.isEmpty()) list else list.filter { q in it.name.lowercase() || q == it.pid.toString() }
        return when (by) {
            ProcessSort.CPU -> shown.sortedWith(compareByDescending<ProcessEntry> { it.cpuPct }.thenByDescending { it.ramBytes })
            ProcessSort.RAM -> shown.sortedByDescending { it.ramBytes }
            ProcessSort.NAME -> shown.sortedBy { it.name.lowercase() }
        }
    }
}

/** `audio.sessions` entry (per-app volume). [id] is opaque. */
data class AudioSession(val id: JsonPrimitive, val name: String, val pid: Long?, val level: Float, val muted: Boolean) {
    companion object {
        fun listFrom(o: JsonObject): List<AudioSession> = o.objList("sessions").mapNotNull { s ->
            AudioSession(
                s["id"] as? JsonPrimitive ?: return@mapNotNull null, s.optStr("name") ?: "App", s.optLong("pid"),
                (s.optDouble("level") ?: 0.0).toFloat().coerceIn(0f, 1f), s.optBool("muted") == true,
            )
        }
    }
}

/** `profiles.get` / `profiles.set` entry. */
data class Profile(val id: String, val name: String, val matchExe: String? = null, val deckId: String? = null, val dockPage: Int? = null) {
    companion object {
        fun listFrom(o: JsonObject): List<Profile> = o.objList("profiles").mapNotNull { p ->
            Profile(
                p.optStr("id") ?: return@mapNotNull null, p.optStr("name") ?: "", p.optStr("match_exe"),
                p.optStr("deck_id"), p.optLong("dock_page")?.toInt(),
            )
        }

        fun encode(list: List<Profile>): JsonObject = buildJsonObject {
            put("profiles", buildJsonArray {
                for (p in list) addJsonObject {
                    put("id", p.id)
                    put("name", p.name)
                    p.matchExe?.takeIf { it.isNotBlank() }?.let { put("match_exe", it) }
                    p.deckId?.takeIf { it.isNotBlank() }?.let { put("deck_id", it) }
                    p.dockPage?.let { put("dock_page", it) }
                }
            })
        }
    }
}

/** `fs.list` entry. */
data class FsEntry(val name: String, val isDir: Boolean, val size: Long, val modifiedMs: Long?) {
    companion object {
        fun listFrom(o: JsonObject): List<FsEntry> = o.objList("entries").mapNotNull { e ->
            FsEntry(e.optStr("name") ?: return@mapNotNull null, e.optBool("is_dir") == true, e.optLong("size") ?: 0, e.optLong("modified_ms"))
        }

        /** Folders first, then by name. */
        fun sorted(list: List<FsEntry>) = list.sortedWith(compareByDescending<FsEntry> { it.isDir }.thenBy { it.name.lowercase() })
    }
}

/** Windows path helpers for the file browser. `""` = the roots list. */
object PcPath {
    fun join(dir: String, name: String): String = when {
        dir.isEmpty() -> name
        dir.endsWith("\\") || dir.endsWith("/") -> dir + name
        else -> "$dir\\$name"
    }

    /** Parent folder; a drive root (`C:\`) or a top-level root goes back to `""`. */
    fun parent(path: String): String {
        val p = path.trimEnd('\\', '/')
        if (p.isEmpty() || isDriveRoot(path)) return ""
        val i = p.lastIndexOfAny(charArrayOf('\\', '/'))
        if (i < 0) return ""
        val head = p.substring(0, i)
        return if (head.length == 2 && head[1] == ':') "$head\\" else head
    }

    fun isDriveRoot(path: String): Boolean = path.trimEnd('\\', '/').let { it.length == 2 && it[1] == ':' }

    fun name(path: String): String = path.trimEnd('\\', '/').substringAfterLast('\\').substringAfterLast('/').ifEmpty { path }

    /** Rejects names that would escape the folder or are invalid on Windows. */
    fun validName(name: String): Boolean {
        val n = name.trim()
        if (n.isEmpty() || n == "." || n == ".." || n.length > 255) return false
        return n.none { it in "\\/:*?\"<>|" || it.code < 32 }
    }
}

/** `relay.get` / `system.info.relay`. */
data class RelayConfig(val enabled: Boolean, val url: String?) {
    companion object {
        fun from(o: JsonObject?): RelayConfig? {
            o ?: return null
            return RelayConfig(o.optBool("enabled") == true, o.optStr("url")?.takeIf { it.isNotBlank() })
        }
    }
}

/** Device tiers (§7). */
object Tiers {
    val ALL = listOf("viewer", "remote", "admin")
}
