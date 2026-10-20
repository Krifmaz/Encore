package app.encore.android.playback

import app.encore.android.R
import java.util.Locale

/**
 * The quality badge the car shows for the song that plays (0.31): one of four badges and the detail for the
 * subtitle line ("FLAC 24/96", "MP3 320k", "Cadenza 24/192").
 *
 * Worked out from the file (format, bitrate, and the sample rate and bit depth track rows carry from 0.31) and
 * from how the song reaches this phone (StreamInfo.transportKind): a Data saver or adaptive AAC stream is lossy
 * whatever the file is, the server's or this phone's Cadenza is Cadenza, a downloaded copy is what was saved.
 * Hi-res: lossless with more than 16 bits or more than 48 kHz, and DSD. The web and desktop use the same rule.
 */
data class CarQuality(val kind: Kind, val detail: String) {
    enum class Kind(val label: String, val icon: Int) {
        LOSSLESS("Lossless", R.drawable.ic_quality_lossless),
        HIRES("Hi-res", R.drawable.ic_quality_hires),
        LOSSY("Lossy", R.drawable.ic_quality_lossy),
        CADENZA("Cadenza", R.drawable.ic_quality_cadenza),
    }

    /** The button's name for screen readers and hosts that show names: "Hi-res: FLAC 24/96". */
    val name: String get() = if (detail.isEmpty()) kind.label else "${kind.label}: $detail"

    companion object {
        private val LOSSY_FORMATS = setOf("mp3", "mp2", "aac", "ogg", "oga", "opus", "webm", "wma", "mpc", "ac3", "vorbis")
        private val LOSSLESS_FORMATS = setOf("flac", "alac", "wav", "wave", "aiff", "aif", "aifc", "ape", "wv", "tta", "dsf", "dff", "wavpack")
        private val DSD_FORMATS = setOf("dsf", "dff")
        private val BITS_RATE = Regex("(\\d+)/(\\d+(?:\\.\\d+)?)")
        private val KBPS = Regex("(\\d+)k\\b")

        /**
         * The badge for a song. [transportKind]: "original" | "flac" | "aac-96" | "cadenza-96000" | "local" ("" before
         * the service knows); [phoneRate]: the rate Cadenza on this phone upsampled to (0 when it did not);
         * [localQuality]: the saved copy's label ("FLAC 16/44.1", "AAC 256k") when it plays. Null: nothing to show
         * (an unknown format).
         */
        fun of(
            format: String,
            bitrate: Int,
            sampleRate: Int,
            bitDepth: Int,
            transportKind: String = "",
            phoneRate: Int = 0,
            localQuality: String = "",
        ): CarQuality? {
            if (transportKind.startsWith("cadenza-")) {
                val rate = transportKind.removePrefix("cadenza-").toIntOrNull() ?: 0
                // the server's Cadenza renders 24-bit FLAC
                return CarQuality(Kind.CADENZA, if (rate > 0) "Cadenza 24/${khz(rate)}" else "Cadenza")
            }
            if (phoneRate > 0) return CarQuality(Kind.CADENZA, "Cadenza ${khz(phoneRate)} kHz")
            if (transportKind.startsWith("aac-")) {
                val kbps = transportKind.removePrefix("aac-").toIntOrNull() ?: 0
                return CarQuality(Kind.LOSSY, if (kbps > 0) "AAC ${kbps}k" else "AAC")
            }
            if (transportKind == "local" && localQuality.isNotBlank()) ofLabel(localQuality)?.let { return it }
            return ofFile(format, bitrate, sampleRate, bitDepth)
        }

        /** From the file alone (what the server streams as it is, or converts to FLAC without loss). */
        fun ofFile(format: String, bitrate: Int, sampleRate: Int, bitDepth: Int): CarQuality? {
            val f = format.trim().lowercase(Locale.ROOT)
            if (f.isEmpty()) return null
            val name = f.uppercase(Locale.ROOT)
            // .m4a / .mp4 hold AAC or ALAC: ALAC has a bit depth (and a lossless bitrate)
            val lossless = when (f) {
                in LOSSLESS_FORMATS -> true
                "m4a", "mp4" -> bitDepth > 0 || bitrate >= 500
                in LOSSY_FORMATS -> false
                else -> return null
            }
            if (!lossless) return CarQuality(Kind.LOSSY, if (bitrate > 0) "$name ${bitrate}k" else name)
            val shown = if (f == "m4a" || f == "mp4") "ALAC" else name
            if (f in DSD_FORMATS) {
                // DSD64 = 64 × 44.1 kHz
                val mult = if (sampleRate > 0) Math.round(sampleRate / 44100.0) else 0L
                return CarQuality(Kind.HIRES, if (mult > 1) "$shown DSD$mult" else shown)
            }
            val hires = bitDepth > 16 || sampleRate > 48000
            val detail = when {
                bitDepth > 0 && sampleRate > 0 -> "$shown $bitDepth/${khz(sampleRate)}"
                sampleRate > 0 -> "$shown ${khz(sampleRate)} kHz"
                bitDepth > 0 -> "$shown $bitDepth-bit"
                else -> shown
            }
            return CarQuality(if (hires) Kind.HIRES else Kind.LOSSLESS, detail)
        }

        /** A downloaded copy's label (OfflineIndex.qualityLabel): "FLAC 24/96", "AAC 256k", "FLAC (converted)". */
        fun ofLabel(label: String): CarQuality? {
            val l = label.trim()
            if (l.isEmpty()) return null
            BITS_RATE.find(l)?.let { m ->
                val bits = m.groupValues[1].toIntOrNull() ?: 0
                val khz = m.groupValues[2].toDoubleOrNull() ?: 0.0
                return CarQuality(if (bits > 16 || khz > 48.0) Kind.HIRES else Kind.LOSSLESS, l)
            }
            if (l.endsWith("(converted)")) return CarQuality(Kind.LOSSLESS, l)
            if (KBPS.containsMatchIn(l)) return CarQuality(Kind.LOSSY, l)
            val first = l.substringBefore(' ').lowercase(Locale.ROOT)
            return when (first) {
                in LOSSLESS_FORMATS -> CarQuality(Kind.LOSSLESS, l)
                in LOSSY_FORMATS -> CarQuality(Kind.LOSSY, l)
                else -> null
            }
        }

        /** 44100 → "44.1", 96000 → "96", 22050 → "22.05". */
        fun khz(hz: Int): String = String.format(Locale.US, "%.3f", hz / 1000.0).trimEnd('0').trimEnd('.')
    }
}
