package com.niranjan.helm.device

import android.content.Context
import com.niranjan.helm.core.DropResult
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.TransferPercent
import com.niranjan.helm.core.fileDrop
import com.niranjan.helm.core.supportsEveryday

/**
 * Quick file drop (everyday §2) for one shared file: a progress notification while uploading,
 * then `file.drop{path, mode}` with the "When a file arrives on the PC" setting.
 */
class FileDropTask private constructor(
    private val context: Context,
    private val conn: HelmConnection,
    private val name: String,
    private val slot: Int,
) {
    private var lastPercent = -1

    fun progress(sent: Long, size: Long) {
        val p = TransferPercent.of(sent, size)
        if (p == lastPercent) return
        lastPercent = p
        PcNotifications.dropProgress(context, slot, name, p)
    }

    /** After `file.upload.end`. Returns the status text ("Opened on PC", …). */
    suspend fun finish(path: String): String {
        val mode = EverydayStore.get(context).settings.value.dropMode
        val text = if (path.isBlank()) {
            DropResult.doneText(mode, null)
        } else {
            try {
                val r = HelmApi(conn).fileDrop(path, mode)
                DropResult.doneText(mode, r.mode)
            } catch (e: HelmException) {
                "Saved in Downloads\\Helm (${if (e.code == "forbidden") "not allowed to open it" else e.message ?: e.code})"
            }
        }
        PcNotifications.dropDone(context, slot, name, text)
        return text
    }

    fun fail(message: String) {
        PcNotifications.dropDone(context, slot, name, message, failed = true)
    }

    companion object {
        /** Null when the PC has no `g6` (the old behaviour: just upload). */
        fun begin(context: Context, conn: HelmConnection, name: String, slot: Int): FileDropTask? {
            if (!conn.supportsEveryday) return null
            return FileDropTask(context.applicationContext, conn, name, slot % 50).also {
                PcNotifications.dropProgress(it.context, it.slot, name, null)
            }
        }
    }
}
