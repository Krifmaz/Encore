package app.encore.android.car

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.Shader
import android.graphics.Typeface
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.SystemClock
import android.text.TextPaint
import android.view.Surface
import androidx.car.app.SurfaceCallback
import androidx.car.app.SurfaceContainer
import app.encore.android.graph
import app.encore.android.playback.CarRules
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * The live lyrics on the car's map surface (Encore Lyrics, a POI car app with MapWithContentTemplate): the
 * lyrics take all of the free area the car reports. The current line as large as it fits (bold, wrapped,
 * centred), the next line at about half that size below it, the lines before it dimmed above when there is
 * room ([CarRules.surface]), over the song's cover blurred and darkened across the whole screen. A new line
 * slides up and fades in (about 0.3 s, up to 30 frames a second); otherwise the thread only checks the
 * position, 10 times a second, without drawing; nothing at all while the screen isn't shown ([setShown]) or
 * there is no surface. A tap on a line plays from its start ([onSeek], main thread).
 */
class LyricsSurface(
    private val context: Context,
    private val onSeek: (Long) -> Unit,
) : SurfaceCallback {
    private val thread = HandlerThread("car-lyrics-surface").apply { start() }
    private val h = Handler(thread.looper)
    private val main = Handler(Looper.getMainLooper())
    private val fetch: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, "car-lyrics-fetch").apply { isDaemon = true } }
    // the cover on its own thread: a slow cover download mustn't hold the lyrics back
    private val coverFetch: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, "car-lyrics-cover").apply { isDaemon = true } }

    // the surface (main thread writes, render thread draws; [lock] keeps a destroyed surface from being drawn on)
    private val lock = Any()
    private var surface: Surface? = null
    private var width = 0
    private var height = 0
    private var dpi = 160
    private val visible = Rect()
    @Volatile private var shown = false
    @Volatile private var released = false

    // render thread only from here on
    private var songFor: String? = null
    private var title = ""
    private var lines: List<CarLine> = emptyList()
    private var texts: List<String> = emptyList()
    private var loading = false
    private val bgDefault = Color.rgb(0x1C, 0x1C, 0x1E)
    private var bgTop = bgDefault
    /** the cover, blurred, at half the surface's size (drawn stretched over all of it) */
    private var cover: Bitmap? = null
    /** bumped when the song, its lines or the area change: the layout is made again without sliding */
    private var gen = 0
    private var laidGen = -1
    private var laidAt = Int.MIN_VALUE
    private var blocks: List<CarRules.Block> = emptyList()
    /** the blocks of the line before, sliding away while [blocks] come in */
    private var leaving: List<CarRules.Block> = emptyList()
    private var changedAt = 0L
    private var running = false
    private var dirty = true

    private val cream = Color.rgb(0xF0, 0xEA, 0xD6)
    private val muted = Color.rgb(0xC9, 0xC2, 0xAE)
    private val bold = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        color = cream
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
        textAlign = Paint.Align.CENTER
    }
    private val regular = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        color = cream
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
        textAlign = Paint.Align.CENTER
    }
    private val shade = Paint()
    private val coverPaint = Paint(Paint.FILTER_BITMAP_FLAG)

    /** The screen is started (true) or stopped: frames only run while it is shown. Main thread. */
    fun setShown(on: Boolean) {
        if (shown == on || released) return
        shown = on
        CarNowPlaying.watch(on)
        h.post { if (on) kick() }
    }

    fun release() {
        if (released) return
        if (shown) CarNowPlaying.watch(false)
        shown = false
        released = true
        synchronized(lock) { surface = null }
        h.removeCallbacksAndMessages(null)
        h.post { cover?.recycle(); cover = null }
        thread.quitSafely()
        fetch.shutdownNow()
        coverFetch.shutdownNow()
    }

    // ---- SurfaceCallback (main thread) -------------------------------------------------------------------------

    override fun onSurfaceAvailable(surfaceContainer: SurfaceContainer) {
        synchronized(lock) {
            surface = surfaceContainer.surface
            width = surfaceContainer.width
            height = surfaceContainer.height
            dpi = surfaceContainer.dpi.takeIf { it > 0 } ?: 160
            if (visible.isEmpty) visible.set(0, 0, width, height)
        }
        h.post { gen++; dirty = true; kick() }
    }

    override fun onVisibleAreaChanged(visibleArea: Rect) {
        synchronized(lock) { visible.set(visibleArea) }
        h.post { gen++; dirty = true; kick() }
    }

    override fun onStableAreaChanged(stableArea: Rect) {}

    override fun onSurfaceDestroyed(surfaceContainer: SurfaceContainer) {
        synchronized(lock) { surface = null }
    }

    /** A tap (car API level 5 and up): the line under it plays from its start. */
    override fun onClick(x: Float, y: Float) {
        h.post {
            val i = lineAtY(y)
            if (i in lines.indices) {
                val ms = lines[i].timeMs
                main.post { if (!released) onSeek(ms) }
            }
        }
    }

    // ---- the frame loop (render thread) ------------------------------------------------------------------------

    private fun kick() {
        if (running) return
        running = true
        h.post(frame)
    }

    private val frame = object : Runnable {
        override fun run() {
            if (released || !shown || synchronized(lock) { surface == null }) { running = false; return }
            val np = CarNowPlaying.state.value
            if (np?.mediaId != songFor) newSong(np)
            val now = SystemClock.uptimeMillis()
            val at = if (lines.isEmpty()) -1 else LyricsCache.lineAt(lines, np?.positionNow() ?: 0L)
            if (at != laidAt || gen != laidGen) {
                // a new line of the same layout slides in; a new song or area just shows
                val slide = gen == laidGen && blocks.isNotEmpty()
                leaving = if (slide) blocks else emptyList()
                blocks = layout(at)
                laidAt = at
                laidGen = gen
                changedAt = now
                dirty = true
            }
            val moving = leaving.isNotEmpty() && now - changedAt < SLIDE_MS
            if (!moving && leaving.isNotEmpty()) { leaving = emptyList(); dirty = true }
            if (dirty || moving) draw(now)
            // ≤ 30 frames a second while a line slides in; otherwise a cheap position check
            h.postDelayed(this, if (moving) FRAME_MS else IDLE_MS)
        }
    }

    private fun newSong(np: NowPlaying?) {
        songFor = np?.mediaId
        title = np?.title ?: ""
        lines = emptyList()
        texts = emptyList()
        gen++
        bgTop = bgDefault
        cover?.recycle()
        cover = null
        dirty = true
        val id = np?.trackId
        val mediaId = np?.mediaId
        loading = id != null
        val uri = np?.artworkUri
        val (w, hgt) = synchronized(lock) { width to height }
        try {
            coverFetch.execute {
                val src = CoverArt.load(context, uri, 300)
                val color = CoverArt.backgroundColor(src)
                val blurred = if (src != null && w > 0 && hgt > 0) {
                    try { CoverArt.blurred(src, (w / 2).coerceAtLeast(1), (hgt / 2).coerceAtLeast(1)) } catch (_: Throwable) { null }
                } else null
                if (blurred !== src) src?.recycle()
                h.post {
                    if (songFor != mediaId || released) { blurred?.recycle(); return@post }
                    bgTop = color
                    cover?.recycle()
                    cover = blurred
                    dirty = true
                    kick()
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
        try {
            fetch.execute {
                val got = if (id != null) LyricsCache.synced(context.graph.api, id) else emptyList()
                h.post {
                    if (songFor != mediaId) return@post
                    lines = got
                    texts = got.map { it.text }
                    loading = false
                    gen++
                    dirty = true
                    kick()
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
    }

    private fun area(): Rect = synchronized(lock) { if (visible.isEmpty) Rect(0, 0, width, height) else Rect(visible) }

    private fun density(): Float = synchronized(lock) { dpi } / 160f

    private fun measureBold(t: String, size: Float): Float { bold.textSize = size; return bold.measureText(t) }

    private fun measureRegular(t: String, size: Float): Float { regular.textSize = size; return regular.measureText(t) }

    /** [CarRules.surface] for line [at] in the free area. */
    private fun layout(at: Int): List<CarRules.Block> {
        if (texts.isEmpty()) return emptyList()
        val a = area()
        return CarRules.surface(texts, at, a.width().toFloat(), a.height().toFloat(), density(), ::measureBold, ::measureRegular)
    }

    private fun lineAtY(y: Float): Int {
        val a = area()
        val slack = 12f * density()
        for (b in blocks) {
            val top = a.top + b.top
            if (y >= top - slack && y <= top + b.fit.height(CarRules.SURFACE_LEADING) + slack) return b.index
        }
        return -1
    }

    private fun draw(now: Long) {
        synchronized(lock) {
            val s = surface ?: return
            if (!s.isValid) return
            val c: Canvas = try {
                s.lockHardwareCanvas()
            } catch (_: Exception) {
                try { s.lockCanvas(null) } catch (_: Exception) { return }
            }
            try {
                paint(c, now)
                dirty = false
            } finally {
                try { s.unlockCanvasAndPost(c) } catch (_: Exception) {}
            }
        }
    }

    /** The cover across the whole screen (the free area and what the car draws over), darkened for the text. */
    private fun background(c: Canvas) {
        val full = RectF(0f, 0f, width.toFloat(), height.toFloat())
        val b = cover
        if (b != null) {
            c.drawBitmap(b, null, full, coverPaint)
            c.drawColor(Color.argb(150, 0, 0, 0))
            shade.shader = LinearGradient(0f, 0f, 0f, full.height(), Color.argb(70, 0, 0, 0), Color.argb(130, 0, 0, 0), Shader.TileMode.CLAMP)
        } else {
            // no cover (yet): its colour into near black
            shade.shader = LinearGradient(0f, 0f, 0f, full.height(), bgTop, Color.rgb(0x10, 0x10, 0x11), Shader.TileMode.CLAMP)
        }
        c.drawRect(full, shade)
    }

    private fun paint(c: Canvas, now: Long) {
        background(c)
        val a = area()
        if (blocks.isEmpty()) {
            note(c, a)
            return
        }
        val t = ((now - changedAt).toFloat() / SLIDE_MS).coerceIn(0f, 1f)
        val ease = 1f - (1f - t) * (1f - t) * (1f - t)
        val sliding = leaving.isNotEmpty() && t < 1f
        val shift = (blocks.firstOrNull { it.current }?.fit?.size ?: 0f) * 0.45f
        if (sliding) for (b in leaving) block(c, a, b, b.alpha * (1f - ease), -shift * ease)
        for (b in blocks) block(c, a, b, if (sliding) b.alpha * ease else b.alpha, if (sliding) shift * (1f - ease) else 0f)
    }

    /** One block: each line centred across the area, its glyphs centred in a line box of size × leading. */
    private fun block(c: Canvas, a: Rect, b: CarRules.Block, alpha: Float, dy: Float) {
        if (alpha <= 0.01f) return
        val p = if (b.current) bold else regular
        p.textSize = b.fit.size
        p.color = if (b.current) cream else muted
        p.alpha = (alpha * 255).toInt().coerceIn(0, 255)
        // the shadow colour is semi-transparent, so its own alpha is used instead of the paint's: scale it here,
        // or dimmed and fading lines keep full dark halos
        val shadow = ((if (b.current) SHADOW_CURRENT else SHADOW_OTHER) * alpha).toInt().coerceIn(0, 255)
        p.setShadowLayer(if (b.current) 10f else 8f, 0f, 2f, Color.argb(shadow, 0, 0, 0))
        val lh = b.fit.size * CarRules.SURFACE_LEADING
        val fm = p.fontMetrics
        val x = a.exactCenterX()
        b.fit.lines.forEachIndexed { i, line ->
            val baseline = a.top + b.top + dy + i * lh + lh / 2f - (fm.ascent + fm.descent) / 2f
            c.drawText(line, x, baseline, p)
        }
    }

    /** No lines to show: the title large and why, in the middle of the free area. */
    private fun note(c: Canvas, a: Rect) {
        val text = when {
            songFor == null -> "Nothing playing. Start something in Encore."
            loading -> "Loading lyrics"
            else -> "No synced lyrics for this song"
        }
        val d = density()
        val w = a.width() * 0.9f
        val titleMax = minOf(a.height() * 0.14f, 56f * d).coerceAtLeast(20f * d)
        val t = if (title.isEmpty()) null else CarRules.fit(title, w, a.height() * 0.4f, 20f * d, titleMax, CarRules.SURFACE_LEADING, 2, ::measureBold)
        val nSize = (t?.size?.times(0.55f) ?: (22f * d)).coerceIn(16f * d, 30f * d)
        val n = CarRules.fit(text, w, nSize * CarRules.SURFACE_LEADING * 2, nSize * 0.8f, nSize, CarRules.SURFACE_LEADING, 2, ::measureRegular)
        val gap = nSize * 0.8f
        val total = (t?.height(CarRules.SURFACE_LEADING)?.plus(gap) ?: 0f) + n.height(CarRules.SURFACE_LEADING)
        var top = (a.height() - total) / 2f
        if (t != null) {
            block(c, a, CarRules.Block(-1, t, top, 1f, true), 1f, 0f)
            top += t.height(CarRules.SURFACE_LEADING) + gap
        }
        block(c, a, CarRules.Block(-1, n, top, 0.75f, false), 0.75f, 0f)
    }

    private companion object {
        const val FRAME_MS = 33L
        const val IDLE_MS = 100L
        const val SLIDE_MS = 300L
        const val SHADOW_CURRENT = 190
        const val SHADOW_OTHER = 170
    }
}
