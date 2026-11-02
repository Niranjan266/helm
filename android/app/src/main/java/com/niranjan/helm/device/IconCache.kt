package com.niranjan.helm.device

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.getIcon
import com.niranjan.helm.core.toHex
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext

/**
 * App icons from `apps.icon`, cached in memory and on disk (`cache/icons/<hash>.png`) per PC,
 * so the dock draws instantly on the next launch. At most 4 icon requests run at once.
 */
class IconCache(context: Context) {
    private val dir = File(context.cacheDir, "icons").apply { mkdirs() }
    private val memory = LruCache<String, Bitmap>(256)
    private val missing = ConcurrentHashMap.newKeySet<String>()
    private val inFlight = ConcurrentHashMap<String, CompletableDeferred<Bitmap?>>()
    private val gate = Semaphore(4)

    private fun key(hostId: String, appId: String, size: Int): String =
        MessageDigest.getInstance("SHA-256").digest("$hostId|$appId|$size".toByteArray()).toHex().take(32)

    fun peek(hostId: String, appId: String, size: Int = SIZE): Bitmap? = memory.get(key(hostId, appId, size))

    suspend fun get(api: HelmApi, hostId: String, appId: String, size: Int = SIZE): Bitmap? =
        fetch(key(hostId, appId, size)) { api.appIcon(appId, size) }

    /** G5 custom deck/dock icon (`c:<hash>`, from `deck.icon.get`). Content-addressed, so never stale. */
    fun peekCustom(hostId: String, icon: String): Bitmap? = memory.get(key(hostId, icon, 0))

    suspend fun getCustom(api: HelmApi, hostId: String, icon: String): Bitmap? =
        fetch(key(hostId, icon, 0)) { api.getIcon(icon) }

    /** Remembers bytes just uploaded so the new icon shows without a round trip. */
    fun putCustom(hostId: String, icon: String, png: ByteArray) {
        val bmp = BitmapFactory.decodeByteArray(png, 0, png.size) ?: return
        val k = key(hostId, icon, 0)
        memory.put(k, bmp)
        missing.remove(k)
        runCatching { File(dir, "$k.png").writeBytes(png) }
    }

    private suspend fun fetch(k: String, download: suspend () -> ByteArray?): Bitmap? {
        memory.get(k)?.let { return it }
        if (k in missing) return null
        val mine = CompletableDeferred<Bitmap?>()
        val existing = inFlight.putIfAbsent(k, mine)
        if (existing != null) return existing.await()
        val result = try {
            load(k, download)
        } catch (_: Exception) {
            null
        }
        inFlight.remove(k)
        mine.complete(result)
        return result
    }

    private suspend fun load(k: String, download: suspend () -> ByteArray?): Bitmap? {
        val file = File(dir, "$k.png")
        val cached = withContext(Dispatchers.IO) { if (file.exists()) BitmapFactory.decodeFile(file.path) else null }
        if (cached != null) {
            memory.put(k, cached)
            return cached
        }
        val png = gate.withPermit { download() }
        if (png == null) {
            missing += k
            return null
        }
        return withContext(Dispatchers.IO) {
            val bmp = BitmapFactory.decodeByteArray(png, 0, png.size) ?: return@withContext null
            runCatching { file.writeBytes(png) }
            memory.put(k, bmp)
            bmp
        }
    }

    companion object {
        const val SIZE = 64
    }
}
