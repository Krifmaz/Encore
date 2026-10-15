package app.encore.android.car

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Shader
import android.graphics.Typeface
import android.net.Uri
import android.text.Layout
import android.text.StaticLayout
import android.text.TextPaint
import app.encore.android.BuildConfig
import java.io.ByteArrayOutputStream
import java.io.File

/**
 * The lyrics card: the now-playing artwork in the car while live lyrics are on (Settings → Playback → In the
 * car → Card). A square picture of the blurred, darkened cover with the lines around the current one: two
 * past lines faded above, the current line large in cream, the next three faded below.
 *
 * Each card is written to `cacheDir/car-lyrics/<version>.jpg` and served by CoverProvider as
 * `content://<app>.covers/lyrics/<version>`, so every line change is a new artwork URI the car loads afresh.
 * Not thread-safe; CarControls uses it from one render thread.
 */
class LyricsCard(private val context: Context) {
    private var bgFor: String? = null
    private var bg: Bitmap? = null
    // milliseconds at start: never repeats a number an earlier run used (cards come far slower than one a millisecond),
    // and stays well under the 20-digit name limit
    private var version = System.currentTimeMillis()

    private val cream = Color.rgb(0xF0, 0xEA, 0xD6)

    private val current = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        color = cream
        textSize = 60f
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
        setShadowLayer(12f, 0f, 2f, Color.argb(160, 0, 0, 0))
    }
    private val faded = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        color = cream
        textSize = 36f
        typeface = Typeface.DEFAULT
    }

    /** A rendered card: the JPEG bytes (for artworkData) and its content:// address (for artworkUri). */
    class Rendered(val bytes: ByteArray, val uri: Uri)

    /**
     * Draws the card for line [at] of [lines] (-1: before the first line) over the cover at [coverUri] (keyed by
     * [coverKey] so the blur is made once per album). Null if it couldn't be written.
     */
    fun render(coverKey: String, coverUri: Uri?, lines: List<CarLine>, at: Int): Rendered? {
        val bmp = Bitmap.createBitmap(SIZE, SIZE, Bitmap.Config.ARGB_8888)
        val c = Canvas(bmp)
        drawBackground(c, coverKey, coverUri)
        drawLines(c, lines, at)
        val out = ByteArrayOutputStream(96 * 1024)
        bmp.compress(Bitmap.CompressFormat.JPEG, 88, out)
        bmp.recycle()
        val bytes = out.toByteArray()
        return try {
            val dir = File(context.cacheDir, DIR).apply { mkdirs() }
            version++
            val name = version.toString()
            val tmp = File(dir, "$name.part")
            tmp.writeBytes(bytes)
            if (!tmp.renameTo(File(dir, "$name.jpg"))) { tmp.delete(); return null }
            prune(dir, name)
            Rendered(bytes, uriFor(name))
        } catch (_: Exception) {
            null
        }
    }

    fun release() {
        bg?.recycle()
        bg = null
        bgFor = null
    }

    private fun drawBackground(c: Canvas, key: String, uri: Uri?) {
        if (bgFor != key || bg == null) {
            bg?.recycle()
            bg = CoverArt.load(context, uri, 300)?.let { src -> CoverArt.blurred(src, SIZE, SIZE).also { if (it !== src) src.recycle() } }
            bgFor = key
        }
        val b = bg
        if (b != null) c.drawBitmap(b, 0f, 0f, null) else c.drawColor(Color.rgb(0x1C, 0x1C, 0x1E))
        // darkened, a little more at the top and bottom where the faded lines are
        c.drawColor(Color.argb(140, 0, 0, 0))
        val shade = Paint().apply {
            shader = LinearGradient(
                0f, 0f, 0f, SIZE.toFloat(),
                intArrayOf(Color.argb(120, 0, 0, 0), Color.argb(0, 0, 0, 0), Color.argb(0, 0, 0, 0), Color.argb(140, 0, 0, 0)),
                floatArrayOf(0f, 0.3f, 0.7f, 1f),
                Shader.TileMode.CLAMP,
            )
        }
        c.drawRect(0f, 0f, SIZE.toFloat(), SIZE.toFloat(), shade)
    }

    private fun layout(text: String, paint: TextPaint, maxLines: Int): StaticLayout =
        StaticLayout.Builder.obtain(text, 0, text.length, paint, SIZE - 2 * MARGIN)
            .setAlignment(Layout.Alignment.ALIGN_NORMAL)
            .setLineSpacing(0f, 1.08f)
            .setMaxLines(maxLines)
            .setEllipsize(android.text.TextUtils.TruncateAt.END)
            .setIncludePad(false)
            .build()

    private fun drawLines(c: Canvas, lines: List<CarLine>, at: Int) {
        val curText = if (at in lines.indices) lines[at].text.ifBlank { "♪" } else "♪"
        val cur = layout(curText, current, 4)
        val gap = 30f
        val curTop = (SIZE - cur.height) / 2f - 20f
        draw(c, cur, curTop, 255)

        // the next lines below, fading out
        var y = curTop + cur.height + gap
        val alphas = intArrayOf(150, 105, 65)
        for (k in 1..3) {
            val i = at + k
            if (i !in lines.indices || y > SIZE - 40) break
            val l = layout(lines[i].text.ifBlank { "♪" }, faded, 2)
            draw(c, l, y, alphas[k - 1])
            y += l.height + gap * 0.6f
        }
        // the past lines above, fading out upwards
        y = curTop - gap
        val pastAlphas = intArrayOf(90, 50)
        for (k in 1..2) {
            val i = at - k
            if (i < 0 || y < 40) break
            val l = layout(lines[i].text.ifBlank { "♪" }, faded, 2)
            y -= l.height
            draw(c, l, y, pastAlphas[k - 1])
            y -= gap * 0.6f
        }
    }

    private fun draw(c: Canvas, l: StaticLayout, top: Float, alpha: Int) {
        l.paint.alpha = alpha
        c.save()
        c.translate(MARGIN.toFloat(), top)
        l.draw(c)
        c.restore()
        l.paint.alpha = 255
    }

    /** Keeps the newest few cards: the car may still be reading the one before the current one. */
    private fun prune(dir: File, current: String) {
        val files = dir.listFiles()?.filter { it.name.endsWith(".jpg") || it.name.endsWith(".part") } ?: return
        if (files.size <= KEEP) return
        // newest by the version in the name (mtimes can tie within a second); the card just written always stays
        files.sortedByDescending { it.name.substringBefore('.').toLongOrNull() ?: -1L }
            .drop(KEEP)
            .filter { it.name.substringBefore('.') != current }
            .forEach { it.delete() }
    }

    companion object {
        const val SIZE = 800
        private const val MARGIN = 56
        private const val KEEP = 6
        const val DIR = "car-lyrics"

        fun uriFor(name: String): Uri = Uri.parse("content://${BuildConfig.APPLICATION_ID}.covers/lyrics/$name")

        /** The card file CoverProvider serves for `lyrics/<name>` (null for anything that isn't a card's name). */
        fun file(context: Context, name: String): File? {
            if (name.isEmpty() || name.length > 20 || !name.all { it.isDigit() }) return null
            return File(File(context.cacheDir, DIR), "$name.jpg").takeIf { it.isFile }
        }
    }
}
