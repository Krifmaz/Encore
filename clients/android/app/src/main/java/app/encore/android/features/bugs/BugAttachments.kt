package app.encore.android.features.bugs

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.os.Build
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.palette
import coil3.ImageLoader
import coil3.compose.AsyncImage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.IOException

/**
 * The pictures of a new report or a reply (the web's "Add images" picker): up to six through the system photo
 * picker. `PickMultipleVisualMedia` opens Android's photo picker (Android 11+ with the updatable picker, or Google
 * Play's copy on older phones) and falls back to the system file chooser where neither exists; the pictures come back
 * readable without any storage permission.
 */
class AttachmentPicker internal constructor(
    val files: SnapshotStateList<Attachment>,
    private val launchPicker: () -> Unit,
    busyState: androidx.compose.runtime.MutableState<Boolean>,
) {
    /** Pictures are being read (or made smaller) after the picker closed. */
    val busy: Boolean by busyState

    /** Opens the photo picker; with six pictures already, says so instead. */
    fun add() {
        if (files.size >= BugRules.MAX_FILES) {
            Toasts.show("At most ${BugRules.MAX_FILES} files per report", "err")
            return
        }
        launchPicker()
    }

    fun remove(index: Int) {
        if (index in files.indices) files.removeAt(index)
    }

    fun clear() = files.clear()
}

@Composable
fun rememberAttachmentPicker(): AttachmentPicker {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val files = remember { mutableStateListOf<Attachment>() }
    val busy = remember { mutableStateOf(false) }
    // the contract wants at least 2; the list is cut to the free places below
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(BugRules.MAX_FILES)) { uris ->
        if (uris.isEmpty()) return@rememberLauncherForActivityResult
        val room = BugRules.MAX_FILES - files.size
        if (uris.size > room) Toasts.show("At most ${BugRules.MAX_FILES} files per report", "err")
        val take = uris.filter { u -> files.none { it.uri == u } }.take(maxOf(0, room))
        if (take.isEmpty()) return@rememberLauncherForActivityResult
        busy.value = true
        scope.launch {
            try {
                for (uri in take) {
                    val a = try {
                        withContext(Dispatchers.IO) { prepareAttachment(context, uri) }
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "That image can't be read", "err")
                        null
                    }
                    if (a != null && files.size < BugRules.MAX_FILES) files.add(a)
                }
            } finally {
                busy.value = false
            }
        }
    }
    return remember(files, launcher) {
        AttachmentPicker(files, { launcher.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }, busy)
    }
}

/** The picked pictures as small squares with a × each (the web's `.bug-thumbs`). */
@Composable
fun AttachmentThumbs(picker: AttachmentPicker, imageLoader: ImageLoader, modifier: Modifier = Modifier) {
    if (picker.files.isEmpty()) return
    val p = MaterialTheme.palette
    Row(modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        picker.files.forEachIndexed { i, f ->
            Box(Modifier.size(64.dp)) {
                AsyncImage(
                    model = f.uri,
                    contentDescription = f.name,
                    imageLoader = imageLoader,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .size(64.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .background(p.panel2)
                        .border(1.dp, p.border, RoundedCornerShape(8.dp)),
                )
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .padding(3.dp)
                        .size(22.dp)
                        .clip(CircleShape)
                        .background(p.bg.copy(alpha = 0.8f))
                        .clickable { picker.remove(i) },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove ${f.name}", tint = p.text, modifier = Modifier.size(14.dp))
                }
            }
        }
    }
}

// ---- reading a picked picture --------------------------------------------------------------------------------------

/** The longest side of a picture that has to be made smaller (a camera photo over 8 MB, a HEIC photo): plenty for a report. */
private const val SHRINK_SIDE = 3072
private const val JPEG_QUALITY = 85

/**
 * A picked picture as it will be sent: as it is when the server takes it (PNG, JPEG, WebP or GIF, at most 8 MB and
 * 8192 pixels a side), otherwise decoded and sent as a JPEG of at most 3072 pixels a side (1536 if that is still
 * over 8 MB). Throws with a sentence when the picture can't be read.
 */
internal fun prepareAttachment(context: Context, uri: Uri): Attachment {
    val resolver = context.contentResolver
    var name = ""
    var size = -1L
    try {
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (c.moveToFirst()) {
                val n = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val s = c.getColumnIndex(OpenableColumns.SIZE)
                if (n >= 0 && !c.isNull(n)) name = c.getString(n) ?: ""
                if (s >= 0 && !c.isNull(s)) size = c.getLong(s)
            }
        }
    } catch (_: Exception) {
    }
    val label = name.ifEmpty { "image" }
    val head = ByteArray(16)
    val read = resolver.openInputStream(uri)?.use { input ->
        var n = 0
        while (n < head.size) {
            val r = input.read(head, n, head.size - n)
            if (r < 0) break
            n += r
        }
        n
    } ?: throw IOException("That image can't be read")
    val format = BugRules.sniffImage(head.copyOf(read))
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
    if (BugRules.sendsAsIs(format, size, bounds.outWidth, bounds.outHeight) && format != null) {
        return Attachment(uri, BugRules.fileName(label, format), BugRules.mimeOf(format), size, null)
    }
    for (side in intArrayOf(SHRINK_SIDE, SHRINK_SIDE / 2)) {
        val jpeg = shrinkToJpeg(context, uri, side, bounds.outWidth, bounds.outHeight)
            ?: throw IOException("That image can't be read")
        if (jpeg.size <= BugRules.MAX_FILE_BYTES) {
            return Attachment(uri, BugRules.fileName(label, "jpg"), "image/jpeg", jpeg.size.toLong(), jpeg)
        }
    }
    throw IOException("$label is over 8 MB")
}

/** The picture decoded upright (EXIF orientation applied), at most [maxSide] a side, as JPEG bytes; null if unreadable. */
private fun shrinkToJpeg(context: Context, uri: Uri, maxSide: Int, knownWidth: Int, knownHeight: Int): ByteArray? {
    val bitmap = try {
        if (Build.VERSION.SDK_INT >= 28) decodeP(context, uri, maxSide) else decodeLegacy(context, uri, maxSide, knownWidth, knownHeight)
    } catch (_: Exception) {
        null
    } catch (_: OutOfMemoryError) {
        null
    } ?: return null
    return try {
        val out = ByteArrayOutputStream()
        if (!bitmap.compress(Bitmap.CompressFormat.JPEG, JPEG_QUALITY, out)) null else out.toByteArray()
    } finally {
        bitmap.recycle()
    }
}

/** Android 9+: ImageDecoder reads HEIC too, turns the picture upright and decodes straight to the smaller size. */
@androidx.annotation.RequiresApi(28)
private fun decodeP(context: Context, uri: Uri, maxSide: Int): Bitmap {
    val source = ImageDecoder.createSource(context.contentResolver, uri)
    return ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
        val (w, h) = BugRules.fitInside(info.size.width, info.size.height, maxSide)
        if (w != info.size.width || h != info.size.height) decoder.setTargetSize(w, h)
        decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        // a JPEG has no alpha: 2 bytes a pixel where the picture allows it
        decoder.memorySizePolicy = ImageDecoder.MEMORY_POLICY_LOW_RAM
    }
}

/** Android 8: BitmapFactory with a power-of-two step down, then the EXIF turn and the last scale by hand. */
private fun decodeLegacy(context: Context, uri: Uri, maxSide: Int, width: Int, height: Int): Bitmap? {
    val resolver = context.contentResolver
    var sample = 1
    if (width > 0 && height > 0) {
        while (maxOf(width, height) / (sample * 2) >= maxSide) sample *= 2
    }
    val opts = BitmapFactory.Options().apply {
        inSampleSize = sample
        inPreferredConfig = Bitmap.Config.RGB_565
    }
    var bmp = resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, opts) } ?: return null
    val degrees = try {
        resolver.openInputStream(uri)?.use { input ->
            when (ExifInterface(input).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
                ExifInterface.ORIENTATION_ROTATE_90 -> 90f
                ExifInterface.ORIENTATION_ROTATE_180 -> 180f
                ExifInterface.ORIENTATION_ROTATE_270 -> 270f
                else -> 0f
            }
        } ?: 0f
    } catch (_: Exception) {
        0f
    }
    val (w, h) = BugRules.fitInside(bmp.width, bmp.height, maxSide)
    if (degrees != 0f || w != bmp.width || h != bmp.height) {
        val m = Matrix()
        m.postScale(w.toFloat() / bmp.width, h.toFloat() / bmp.height)
        if (degrees != 0f) m.postRotate(degrees)
        val turned = Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, m, true)
        if (turned !== bmp) bmp.recycle()
        bmp = turned
    }
    return bmp
}
