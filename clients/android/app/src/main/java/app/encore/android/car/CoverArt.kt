package app.encore.android.car

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.net.Uri

/** Covers for the car's lyrics views: loaded from CoverProvider's content:// URIs, blurred, or reduced to a colour. */
object CoverArt {
    /** The cover at [uri], decoded no larger than about [maxPx] on its longer side. Null when it can't be read. Blocking. */
    fun load(context: Context, uri: Uri?, maxPx: Int = 600): Bitmap? {
        if (uri == null) return null
        return try {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
            var sample = 1
            val longer = maxOf(bounds.outWidth, bounds.outHeight)
            while (longer / (sample * 2) >= maxPx) sample *= 2
            val opts = BitmapFactory.Options().apply { inSampleSize = sample }
            context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, opts) }
        } catch (_: Exception) {
            null
        } catch (_: OutOfMemoryError) {
            null
        }
    }

    /**
     * A soft, blurred copy of [src] at [w]×[h] (center-cropped): shrunk to a few dozen pixels, box-blurred
     * three times (close to a gaussian) and stretched back with filtering.
     */
    fun blurred(src: Bitmap, w: Int, h: Int): Bitmap {
        val small = 48
        val sw: Int
        val sh: Int
        if (w >= h) { sw = small; sh = maxOf(1, small * h / w) } else { sh = small; sw = maxOf(1, small * w / h) }
        // center crop to the target aspect first
        val srcAspect = src.width.toFloat() / src.height
        val dstAspect = w.toFloat() / h
        val crop = if (srcAspect > dstAspect) {
            val cw = (src.height * dstAspect).toInt().coerceIn(1, src.width)
            Bitmap.createBitmap(src, (src.width - cw) / 2, 0, cw, src.height)
        } else {
            val ch = (src.width / dstAspect).toInt().coerceIn(1, src.height)
            Bitmap.createBitmap(src, 0, (src.height - ch) / 2, src.width, ch)
        }
        val tiny = Bitmap.createScaledBitmap(crop, sw, sh, true)
        val px = IntArray(sw * sh)
        tiny.getPixels(px, 0, sw, 0, 0, sw, sh)
        repeat(3) { boxBlur(px, sw, sh, 3) }
        val out = Bitmap.createBitmap(sw, sh, Bitmap.Config.ARGB_8888)
        out.setPixels(px, 0, sw, 0, 0, sw, sh)
        return Bitmap.createScaledBitmap(out, w, h, true)
    }

    /** The cover's average colour, pulled darker so cream text stays readable on it. */
    fun backgroundColor(src: Bitmap?): Int {
        if (src == null) return Color.rgb(0x1C, 0x1C, 0x1E)
        val one = Bitmap.createScaledBitmap(src, 8, 8, true)
        var r = 0L
        var g = 0L
        var b = 0L
        for (y in 0 until 8) for (x in 0 until 8) {
            val c = one.getPixel(x, y)
            r += Color.red(c); g += Color.green(c); b += Color.blue(c)
        }
        val hsv = FloatArray(3)
        Color.RGBToHSV((r / 64).toInt(), (g / 64).toInt(), (b / 64).toInt(), hsv)
        hsv[1] = (hsv[1] * 0.8f).coerceAtMost(0.6f)
        hsv[2] = hsv[2].coerceIn(0.12f, 0.30f)
        return Color.HSVToColor(hsv)
    }

    /** One horizontal + one vertical box blur pass of radius [r], in place. */
    private fun boxBlur(px: IntArray, w: Int, h: Int, r: Int) {
        val tmp = IntArray(px.size)
        for (y in 0 until h) for (x in 0 until w) {
            var a = 0; var rr = 0; var gg = 0; var bb = 0; var n = 0
            for (k in -r..r) {
                val c = px[y * w + (x + k).coerceIn(0, w - 1)]
                a += c ushr 24; rr += (c shr 16) and 0xFF; gg += (c shr 8) and 0xFF; bb += c and 0xFF; n++
            }
            tmp[y * w + x] = ((a / n) shl 24) or ((rr / n) shl 16) or ((gg / n) shl 8) or (bb / n)
        }
        for (y in 0 until h) for (x in 0 until w) {
            var a = 0; var rr = 0; var gg = 0; var bb = 0; var n = 0
            for (k in -r..r) {
                val c = tmp[(y + k).coerceIn(0, h - 1) * w + x]
                a += c ushr 24; rr += (c shr 16) and 0xFF; gg += (c shr 8) and 0xFF; bb += c and 0xFF; n++
            }
            px[y * w + x] = ((a / n) shl 24) or ((rr / n) shl 16) or ((gg / n) shl 8) or (bb / n)
        }
    }
}
