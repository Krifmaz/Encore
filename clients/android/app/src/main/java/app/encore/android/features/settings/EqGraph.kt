package app.encore.android.features.settings

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.cadenza.CadenzaEngine
import app.encore.android.data.prefs.EqBand
import app.encore.android.ui.theme.palette
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.double
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.math.ln
import kotlin.math.pow
import kotlin.math.roundToInt

// The equalizer as a graph (Settings → Playback → Sound effects), from Cadenza's Sound page: each band's curve faint,
// the sum bold and filled to 0 dB, a numbered handle per band. Drag a handle (left/right = frequency, up/down = gain),
// double-tap the empty graph for a bell. The curves come from Cadenza itself (CadenzaEngine.eqCurve), so the graph
// shows exactly what plays.

private const val FMIN = 20.0
private const val FMAX = 20_000.0
private const val POINTS = 200
private val COLORS = listOf(0xFFF97316, 0xFFEAB308, 0xFF22C55E, 0xFF06B6D4, 0xFF3B82F6, 0xFFA855F7, 0xFFEC4899, 0xFFEF4444).map { Color(it) }

/** Bands with a gain (bells and shelves); the cuts and the notch have none. */
private fun hasGain(b: EqBand) = b.type == "peaking" || b.type == "lowshelf" || b.type == "highshelf"

/** ±12 dB, or ±24 when a band goes past 12. */
private fun rangeOf(bands: List<EqBand>) = if (bands.any { kotlin.math.abs(it.gain) > 12.0 }) 24.0 else 12.0

private class Curve(val freqs: DoubleArray, val total: DoubleArray, val bands: List<DoubleArray>, val handles: DoubleArray)

private fun bandsJson(bands: List<EqBand>): String = JsonArray(bands.map { b ->
    JsonObject(mapOf("type" to JsonPrimitive(b.type), "freq" to JsonPrimitive(b.freq), "gain" to JsonPrimitive(b.gain), "q" to JsonPrimitive(b.q)))
}).toString()

private fun curveOf(bands: List<EqBand>): Curve? = try {
    val o = Json.parseToJsonElement(CadenzaEngine.eqCurve(bandsJson(bands), POINTS)).jsonObject
    fun arr(e: JsonElement?) = e!!.jsonArray.map { it.jsonPrimitive.double }.toDoubleArray()
    Curve(arr(o["freqs"]), arr(o["total"]), o["bands"]!!.jsonArray.map { arr(it) }, arr(o["handles"]))
} catch (_: Throwable) {
    null
}

private fun hzLabel(f: Double): String =
    if (f >= 1000) "${(f / 1000).let { if (it == it.roundToInt().toDouble()) it.roundToInt().toString() else it.toString() }}k" else f.roundToInt().toString()

/**
 * The graph for [bands]. While a handle is dragged the graph follows the finger; [onChange] gets the new list when it
 * is let go (and when a bell is added). [enabled] false draws it dimmed and flat, and takes no input.
 */
@Composable
fun EqGraph(bands: List<EqBand>, enabled: Boolean, maxBands: Int, onChange: (List<EqBand>) -> Unit) {
    val p = MaterialTheme.palette
    val measurer = rememberTextMeasurer()
    var live by remember(bands) { mutableStateOf(bands) }
    var sel by remember { mutableStateOf(-1) }
    val curve = remember(live) { curveOf(live) } ?: return
    val liveNow by rememberUpdatedState(live)
    val curveNow by rememberUpdatedState(curve)
    val commit by rememberUpdatedState(onChange)
    val shape = RoundedCornerShape(12.dp)

    Canvas(
        Modifier.fillMaxWidth().height(200.dp).clip(shape).background(p.bg2).border(1.dp, p.border, shape)
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    val (w, h) = size.width.toFloat() to size.height.toFloat()
                    val pad = 14.dp.toPx()
                    val range = rangeOf(liveNow)
                    val fx = { f: Double -> (ln(f / FMIN) / ln(FMAX / FMIN) * w).toFloat() }
                    val gy = { g: Double -> (h / 2 - g / range * (h / 2 - pad)).toFloat() }
                    // the handle under the finger (the last drawn wins), else let the page scroll
                    val i = liveNow.indices.reversed().firstOrNull { k ->
                        val c = Offset(fx(liveNow[k].freq.toDouble().coerceAtLeast(FMIN)), gy(curveNow.handles.getOrElse(k) { 0.0 }.coerceIn(-range, range)))
                        (c - down.position).getDistance() <= 24.dp.toPx()
                    } ?: return@awaitEachGesture
                    down.consume()
                    sel = i
                    var cur = liveNow
                    drag(down.id) { change ->
                        val x = change.position.x.coerceIn(0f, w)
                        val f = FMIN * (FMAX / FMIN).pow((x / w).toDouble())
                        val g = ((h / 2 - change.position.y) / (h / 2 - pad) * range).coerceIn(-range, range)
                        val b = cur[i]
                        val nb = b.copy(freq = f.roundToInt().coerceIn(20, 20000), gain = if (hasGain(b)) (g * 2).roundToInt() / 2.0 else b.gain)
                        cur = cur.toMutableList().also { it[i] = nb }
                        live = cur
                        change.consume()
                    }
                    commit(cur)
                }
            }
            .pointerInput(enabled, maxBands) {
                if (!enabled) return@pointerInput
                detectTapGestures(onDoubleTap = { pos ->
                    if (liveNow.size >= maxBands) return@detectTapGestures
                    val (w, h) = size.width.toFloat() to size.height.toFloat()
                    val range = rangeOf(liveNow)
                    val f = FMIN * (FMAX / FMIN).pow((pos.x / w).toDouble().coerceIn(0.0, 1.0))
                    val g = ((h / 2 - pos.y) / (h / 2 - 14.dp.toPx()) * range).coerceIn(-range, range)
                    commit(liveNow + EqBand("peaking", f.roundToInt().coerceIn(20, 20000), (g * 2).roundToInt() / 2.0, 1.0))
                })
            },
    ) {
        val (w, h) = size.width to size.height
        val pad = 14.dp.toPx()
        val range = rangeOf(live)
        fun fx(f: Double) = (ln(f / FMIN) / ln(FMAX / FMIN) * w).toFloat()
        fun gy(g: Double) = (h / 2 - g / range * (h / 2 - pad)).toFloat().coerceIn(-50f, h + 50f)
        val label = TextStyle(color = p.faint, fontSize = 10.sp)
        for (f in listOf(20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0)) {
            val x = fx(f)
            drawLine(p.border, Offset(x, 0f), Offset(x, h), 1f)
            if (f < 20000.0) drawText(measurer, hzLabel(f), Offset(x + 3f, h - 14.dp.toPx()), label)
        }
        for (g in listOf(-range, -range / 2, 0.0, range / 2, range)) {
            val y = gy(g)
            drawLine(p.border, Offset(0f, y), Offset(w, y), if (g == 0.0) 2f else 1f)
        }
        fun path(db: DoubleArray) = Path().apply {
            curve.freqs.forEachIndexed { k, f -> if (k == 0) moveTo(fx(f), gy(db[k])) else lineTo(fx(f), gy(db[k])) }
        }
        if (enabled) {
            curve.bands.forEachIndexed { i, db ->
                drawPath(path(db), COLORS[i % COLORS.size].copy(alpha = if (sel == i) 0.9f else 0.35f), style = Stroke(1.2.dp.toPx()))
            }
        }
        val total = if (enabled) curve.total else DoubleArray(curve.freqs.size)
        val zero = gy(0.0)
        val fill = path(total).apply { lineTo(w, zero); lineTo(0f, zero); close() }
        drawPath(fill, p.accent.copy(alpha = 0.18f))
        drawPath(path(total), p.accent, style = Stroke(2.4.dp.toPx()))
        live.forEachIndexed { i, b ->
            val c = Offset(fx(b.freq.toDouble().coerceAtLeast(FMIN)), gy(curve.handles.getOrElse(i) { 0.0 }.coerceIn(-range, range)))
            val r = (if (sel == i) 10 else 8).dp.toPx()
            drawCircle(p.bg2, r + 2.dp.toPx(), c)
            drawCircle(COLORS[i % COLORS.size].copy(alpha = if (enabled) 0.95f else 0.4f), r, c)
            val t = measurer.measure("${i + 1}", TextStyle(color = p.bg, fontSize = 10.sp, fontWeight = FontWeight.Bold))
            drawText(t, topLeft = Offset(c.x - t.size.width / 2f, c.y - t.size.height / 2f))
        }
    }
}
