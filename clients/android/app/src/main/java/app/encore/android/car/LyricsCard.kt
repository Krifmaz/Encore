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
import android.text.TextPaint
import app.encore.android.BuildConfig
import app.encore.android.playback.CarRules
import java.io.ByteArrayOutputStream
import java.io.File

/**
 * The lyrics card: the now-playing artwork in the car while live lyrics are on (Settings → Playback → In the
 * car → Card). A square picture ([SIZE] px, sharp in a car's cover box) of the blurred, darkened cover with the
 * current line as large as it fits (bold, wrapped, centred) and the next line smaller below it, nothing else
 * ([CarRules.card]). Android Auto draws its own player screen and decides how big the cover box is; the card
 * makes the most of that box.
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
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
        textAlign = Paint.Align.CENTER
        setShadowLayer(16f, 0f, 3f, Color.argb(200, 0, 0, 0))
    }
    private val faded = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        color = cream
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
        textAlign = Paint.Align.CENTER
        setShadowLayer(12f, 0f, 2f, Color.argb(190, 0, 0, 0))
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
        bmp.compress(Bitmap.CompressFormat.JPEG, 85, out)
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
        // darkened evenly (the text sits in the middle), a little more towards the edges
        c.drawColor(Color.argb(150, 0, 0, 0))
        val shade = Paint().apply {
            shader = LinearGradient(
                0f, 0f, 0f, SIZE.toFloat(),
                intArrayOf(Color.argb(90, 0, 0, 0), Color.argb(0, 0, 0, 0), Color.argb(0, 0, 0, 0), Color.argb(110, 0, 0, 0)),
                floatArrayOf(0f, 0.25f, 0.75f, 1f),
                Shader.TileMode.CLAMP,
            )
        }
        c.drawRect(0f, 0f, SIZE.toFloat(), SIZE.toFloat(), shade)
    }

    /** The current line ([at]; before the first line a note) and the next one, laid out by [CarRules.card]. */
    private fun drawLines(c: Canvas, lines: List<CarLine>, at: Int) {
        val curText = if (at in lines.indices) lines[at].text else ""
        val nextText = lines.getOrNull(at + 1)?.text
        val card = CarRules.card(
            curText,
            nextText,
            { t, size -> current.textSize = size; current.measureText(t) },
            { t, size -> faded.textSize = size; faded.measureText(t) },
        )
        drawFit(c, card.current, card.currentTop, current, 255)
        card.next?.let { drawFit(c, it, card.nextTop, faded, 190) }
    }

    /** Each line centred on the card, its glyphs centred in a line box of size × leading. */
    private fun drawFit(c: Canvas, fit: CarRules.Fit, top: Float, paint: TextPaint, alpha: Int) {
        paint.textSize = fit.size
        paint.alpha = alpha
        val lh = fit.size * CarRules.CARD_LEADING
        val fm = paint.fontMetrics
        val x = SIZE / 2f
        fit.lines.forEachIndexed { i, line ->
            val baseline = top + i * lh + lh / 2f - (fm.ascent + fm.descent) / 2f
            c.drawText(line, x, baseline, paint)
        }
        paint.alpha = 255
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
        const val SIZE = CarRules.CARD_SIZE
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
