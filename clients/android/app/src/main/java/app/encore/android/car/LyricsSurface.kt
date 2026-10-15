package app.encore.android.car

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Rect
import android.graphics.Shader
import android.graphics.Typeface
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.SystemClock
import android.text.Layout
import android.text.StaticLayout
import android.text.TextPaint
import android.text.TextUtils
import android.view.Surface
import androidx.car.app.SurfaceCallback
import androidx.car.app.SurfaceContainer
import app.encore.android.graph
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import kotlin.math.abs
import kotlin.math.exp

/**
 * The live lyrics on the car's map surface (Encore Lyrics, a POI car app with MapWithContentTemplate): the
 * song's synced lines scrolling smoothly, the current one larger in cream, the others fading with distance,
 * over a background in the cover's colour. Draws on its own thread at up to 30 frames a second while a line
 * moves, and only checks the position (10 times a second, no drawing) when nothing moves; nothing at all while
 * the screen isn't shown ([setShown]) or there is no surface. A tap on a line seeks to it ([onSeek], main
 * thread); a drag scrolls the lines by hand for a few seconds.
 */
class LyricsSurface(
    private val context: Context,
    private val onSeek: (Long) -> Unit,
) : SurfaceCallback {
    private val thread = HandlerThread("car-lyrics-surface").apply { start() }
    private val h = Handler(thread.looper)
    private val main = Handler(Looper.getMainLooper())
    private val fetch: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, "car-lyrics-fetch").apply { isDaemon = true } }
    // the cover colour on its own thread: a slow cover download mustn't hold the lyrics back
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
    private var artist = ""
    private var lines: List<CarLine> = emptyList()
    private var loading = false
    private val bgDefault = Color.rgb(0x1C, 0x1C, 0x1E)
    private var bgTop = bgDefault
    private var layouts: List<StaticLayout> = emptyList()
    private var tops = FloatArray(0)
    private var emphasis = FloatArray(0)
    private var laidFor: Any? = null
    private var laidLines: List<CarLine>? = null
    private var lineAt = -2
    private var scroll = Float.NaN
    private var manual = 0f
    private var manualUntil = 0L
    private var lastFrame = 0L
    private var running = false
    private var dirty = true

    private val cream = Color.rgb(0xF0, 0xEA, 0xD6)
    private val muted = Color.rgb(0xAA, 0xA4, 0x93)
    private val linePaint = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        color = cream
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
    }
    private val notePaint = TextPaint(Paint.ANTI_ALIAS_FLAG).apply { color = muted }
    private val titlePaint = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        color = cream
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
    }
    private val shade = Paint()

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
        h.post { laidFor = null; dirty = true; kick() }
    }

    override fun onVisibleAreaChanged(visibleArea: Rect) {
        synchronized(lock) { visible.set(visibleArea) }
        h.post { laidFor = null; dirty = true; kick() }
    }

    override fun onStableAreaChanged(stableArea: Rect) {}

    override fun onSurfaceDestroyed(surfaceContainer: SurfaceContainer) {
        synchronized(lock) { surface = null }
    }

    override fun onScroll(distanceX: Float, distanceY: Float) {
        h.post {
            manual += distanceY
            manualUntil = SystemClock.uptimeMillis() + 4000L
            kick()
        }
    }

    /** A tap (car API level 5 and up): the line under it plays from its start. */
    override fun onClick(x: Float, y: Float) {
        h.post {
            val i = lineAtY(y)
            if (i >= 0) {
                val ms = lines[i].timeMs
                manual = 0f
                main.post { if (!released) onSeek(ms) }
            }
        }
    }

    // ---- the frame loop (render thread) ------------------------------------------------------------------------

    private fun kick() {
        if (running) return
        running = true
        lastFrame = SystemClock.uptimeMillis()
        h.post(frame)
    }

    private val frame = object : Runnable {
        override fun run() {
            if (released || !shown || synchronized(lock) { surface == null }) { running = false; return }
            val np = CarNowPlaying.state.value
            if (np?.mediaId != songFor) newSong(np)
            relayout()
            val now = SystemClock.uptimeMillis()
            val dt = (now - lastFrame).coerceIn(1L, 100L)
            lastFrame = now
            val k = (1.0 - exp(-dt / 140.0)).toFloat()

            val at = if (lines.isEmpty()) -1 else LyricsCache.lineAt(lines, np?.positionNow() ?: 0L)
            if (at != lineAt) { lineAt = at; dirty = true }
            if (manual != 0f && now > manualUntil) manual = 0f
            val target = targetScroll(at) + manual
            var moving = false
            if (scroll.isNaN()) { scroll = target; dirty = true }
            if (abs(target - scroll) > 0.5f) { scroll += (target - scroll) * k; moving = true; dirty = true } else if (scroll != target) { scroll = target; dirty = true }
            for (i in emphasis.indices) {
                val want = if (i == at) 1f else 0f
                val e = emphasis[i]
                if (abs(want - e) > 0.01f) { emphasis[i] = e + (want - e) * k; moving = true; dirty = true } else if (e != want) { emphasis[i] = want; dirty = true }
            }
            if (dirty) draw()
            // ≤ 30 frames a second while something moves; otherwise a cheap position check
            h.postDelayed(this, if (moving) FRAME_MS else IDLE_MS)
        }
    }

    private fun newSong(np: NowPlaying?) {
        songFor = np?.mediaId
        title = np?.title ?: ""
        artist = np?.artist ?: ""
        lines = emptyList()
        laidFor = null
        lineAt = -2
        scroll = Float.NaN
        manual = 0f
        bgTop = bgDefault
        dirty = true
        val id = np?.trackId
        val mediaId = np?.mediaId
        loading = id != null
        val uri = np?.artworkUri
        try {
            coverFetch.execute {
                val color = CoverArt.backgroundColor(CoverArt.load(context, uri, 200))
                h.post { if (songFor == mediaId) { bgTop = color; dirty = true; kick() } }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
        try {
            fetch.execute {
                val got = if (id != null) LyricsCache.synced(context.graph.api, id) else emptyList()
                h.post {
                    if (songFor != mediaId) return@post
                    lines = got
                    loading = false
                    laidFor = null
                    lineAt = -2
                    scroll = Float.NaN
                    dirty = true
                    kick()
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
    }

    private fun area(): Rect = synchronized(lock) { if (visible.isEmpty) Rect(0, 0, width, height) else Rect(visible) }

    private fun density(): Float = synchronized(lock) { dpi } / 160f

    /** Lays the lines out for the visible area's width (again when it or the song changes). */
    private fun relayout() {
        val a = area()
        val d = density()
        val size = (a.height() / 9f).coerceIn(18f * d, 40f * d)
        val key = a.width() to size
        if (laidFor == key && laidLines === lines) return
        laidFor = key
        laidLines = lines
        linePaint.textSize = size
        notePaint.textSize = size * 0.7f
        titlePaint.textSize = size * 1.1f
        val margin = a.width() * 0.06f
        // room for the current line's 12 % enlargement
        val w = ((a.width() - 2 * margin) / MAX_SCALE).toInt().coerceAtLeast(1)
        layouts = lines.map { layout(it.text.ifBlank { "♪" }, linePaint, w, 3) }
        tops = FloatArray(layouts.size)
        var y = 0f
        val gap = size * 0.7f
        for (i in layouts.indices) { tops[i] = y; y += layouts[i].height + gap }
        emphasis = FloatArray(layouts.size)
        dirty = true
    }

    private fun layout(text: String, paint: TextPaint, width: Int, maxLines: Int): StaticLayout =
        StaticLayout.Builder.obtain(text, 0, text.length, paint, width)
            .setAlignment(Layout.Alignment.ALIGN_NORMAL)
            .setLineSpacing(0f, 1.05f)
            .setMaxLines(maxLines)
            .setEllipsize(TextUtils.TruncateAt.END)
            .setIncludePad(false)
            .build()

    /** The content y to put at the anchor (a little above the middle of the visible area). */
    private fun targetScroll(at: Int): Float {
        if (layouts.isEmpty()) return 0f
        if (at < 0) return -linePaint.textSize * 2
        val i = at.coerceAtMost(layouts.size - 1)
        return tops[i] + layouts[i].height / 2f
    }

    private fun anchorY(a: Rect): Float = a.top + a.height() * 0.42f

    private fun lineAtY(y: Float): Int {
        if (layouts.isEmpty() || scroll.isNaN()) return -1
        val a = area()
        val off = anchorY(a) - scroll
        for (i in layouts.indices) {
            val top = off + tops[i]
            if (y >= top - linePaint.textSize * 0.35f && y <= top + layouts[i].height + linePaint.textSize * 0.35f) return i
        }
        return -1
    }

    private fun draw() {
        synchronized(lock) {
            val s = surface ?: return
            if (!s.isValid) return
            val c: Canvas = try {
                s.lockHardwareCanvas()
            } catch (_: Exception) {
                try { s.lockCanvas(null) } catch (_: Exception) { return }
            }
            try {
                paint(c)
                dirty = false
            } finally {
                try { s.unlockCanvasAndPost(c) } catch (_: Exception) {}
            }
        }
    }

    private fun paint(c: Canvas) {
        val a = area()
        val full = Rect(0, 0, width, height)
        // the cover's colour into near black
        shade.shader = LinearGradient(0f, 0f, 0f, full.height().toFloat(), bgTop, Color.rgb(0x10, 0x10, 0x11), Shader.TileMode.CLAMP)
        c.drawRect(full, shade)
        val margin = a.left + a.width() * 0.06f

        if (layouts.isEmpty()) {
            val note = when {
                songFor == null -> "Nothing playing. Start something in Encore."
                loading -> "Loading lyrics"
                else -> "No synced lyrics for this song"
            }
            val w = (a.width() - 2 * a.width() * 0.06f).toInt().coerceAtLeast(1)
            val t = if (title.isNotEmpty()) layout(title, titlePaint, w, 2) else null
            val n = layout(note, notePaint, w, 2)
            val total = (t?.height ?: 0) + n.height + linePaint.textSize * 0.5f
            var y = a.top + (a.height() - total) / 2f
            if (t != null) {
                c.save(); c.translate(margin, y); t.draw(c); c.restore()
                y += t.height + linePaint.textSize * 0.5f
            }
            c.save(); c.translate(margin, y); n.draw(c); c.restore()
            return
        }

        val off = anchorY(a) - scroll
        val fadeZone = a.height() * 0.18f
        for (i in layouts.indices) {
            val l = layouts[i]
            val top = off + tops[i]
            if (top > a.bottom || top + l.height * MAX_SCALE < a.top) continue
            val e = emphasis.getOrElse(i) { 0f }
            // distance from the anchor fades the context lines; the visible edges fade everything
            val mid = top + l.height / 2f
            val dist = abs(mid - anchorY(a)) / a.height()
            var alpha = (0.55f - dist * 0.9f).coerceIn(0.12f, 0.55f)
            alpha += (1f - alpha) * e
            val edge = minOf((mid - a.top) / fadeZone, (a.bottom - mid) / fadeZone).coerceIn(0f, 1f)
            alpha *= edge
            if (alpha <= 0.01f) continue
            l.paint.color = if (e > 0.5f) cream else blend(muted, cream, e * 2f)
            l.paint.alpha = (alpha * 255).toInt()
            val scale = 1f + (MAX_SCALE - 1f) * e
            c.save()
            c.translate(margin, top)
            c.scale(scale, scale, 0f, l.height / 2f)
            l.draw(c)
            c.restore()
        }
        linePaint.color = cream
        linePaint.alpha = 255
    }

    private fun blend(a: Int, b: Int, t: Float): Int {
        val u = t.coerceIn(0f, 1f)
        return Color.rgb(
            (Color.red(a) + (Color.red(b) - Color.red(a)) * u).toInt(),
            (Color.green(a) + (Color.green(b) - Color.green(a)) * u).toInt(),
            (Color.blue(a) + (Color.blue(b) - Color.blue(a)) * u).toInt(),
        )
    }

    private companion object {
        const val FRAME_MS = 33L
        const val IDLE_MS = 100L
        const val MAX_SCALE = 1.12f
    }
}
