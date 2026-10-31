package com.niranjan.helm.device

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import com.niranjan.helm.core.toHex
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest

/** A file the user picked or shared, ready to upload. */
data class LocalFile(val uri: Uri, val name: String, val size: Long, val sha256: String)

/** Something shared into Helm from another app. */
sealed interface SharedItem {
    data class File(val uri: Uri) : SharedItem
    data class Text(val text: String) : SharedItem {
        val isUrl: Boolean get() = text.trim().let { (it.startsWith("http://") || it.startsWith("https://")) && ' ' !in it }
    }
}

/** Reading content URIs and writing into the phone's gallery / Downloads. */
class PhoneFiles(private val context: Context) {
    private val resolver get() = context.contentResolver

    fun open(uri: Uri): InputStream = resolver.openInputStream(uri) ?: error("Cannot open $uri")

    /** Name, real size and SHA-256 (reads the file once). Runs on a background thread. */
    fun describe(uri: Uri): LocalFile {
        var name: String? = null
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) name = c.getString(0)
        }
        val digest = MessageDigest.getInstance("SHA-256")
        var size = 0L
        open(uri).use { input ->
            val buf = ByteArray(64 * 1024)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                digest.update(buf, 0, n)
                size += n
            }
        }
        val fallback = uri.lastPathSegment?.substringAfterLast('/') ?: "file"
        return LocalFile(uri, sanitize(name ?: fallback), size, digest.digest().toHex())
    }

    private fun sanitize(name: String) = name.replace(Regex("[\\\\/:*?\"<>|]"), "_").ifBlank { "file" }

    /** Saves a PNG into Pictures/Helm. Returns a short description of where it went. */
    fun savePngToGallery(png: ByteArray, displayName: String): String {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val values = ContentValues().apply {
                put(MediaStore.Images.Media.DISPLAY_NAME, displayName)
                put(MediaStore.Images.Media.MIME_TYPE, "image/png")
                put(MediaStore.Images.Media.RELATIVE_PATH, Environment.DIRECTORY_PICTURES + "/Helm")
            }
            val uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) ?: error("Gallery refused the image")
            resolver.openOutputStream(uri)!!.use { it.write(png) }
            return "Pictures/Helm"
        }
        val dir = File(context.getExternalFilesDir(Environment.DIRECTORY_PICTURES), "Helm").apply { mkdirs() }
        File(dir, displayName).writeBytes(png)
        return dir.path
    }

    /**
     * Opens an output for a file received from the PC in Downloads/Helm (app storage on
     * Android 9 and older). [commit] must be called after a successful write; [abort] otherwise.
     */
    fun createDownload(displayName: String): Sink {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val values = ContentValues().apply {
                put(MediaStore.Downloads.DISPLAY_NAME, displayName)
                put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/Helm")
                put(MediaStore.Downloads.IS_PENDING, 1)
            }
            val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: error("Downloads refused the file")
            val out = resolver.openOutputStream(uri)!!
            return Sink(out, "Downloads/Helm/$displayName", commit = {
                resolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
            }, abort = { resolver.delete(uri, null, null) })
        }
        val dir = File(context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS), "Helm").apply { mkdirs() }
        val f = File(dir, displayName)
        return Sink(f.outputStream(), f.path, commit = {}, abort = { f.delete() })
    }

    class Sink(val out: OutputStream, val where: String, val commit: () -> Unit, val abort: () -> Unit)
}
