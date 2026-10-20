package app.encore.android.playback

import java.util.Locale

/**
 * The quality of the song that plays, for the car's subtitle line (the Signal path in a few words): its kind and
 * the detail ("FLAC 24/96", "AAC 256", "FLAC → AAC 96", "FLAC → Cadenza 192 kHz").
 *
 * Worked out from the file (format, bitrate, and the sample rate and bit depth track rows carry from 0.31) and
 * from how the song reaches this phone (StreamInfo.transportKind): a Data saver or adaptive AAC stream is lossy
 * whatever the file is, the server's or this phone's Cadenza is Cadenza, a downloaded copy is what was saved.
 * Hi-res: lossless with more than 16 bits or more than 48 kHz, and DSD. The web and desktop use the same rule.
 */
data class CarQuality(val kind: Kind, val detail: String) {
    enum class Kind { LOSSLESS, HIRES, LOSSY, CADENZA }

    companion object {
        private val LOSSY_FORMATS = setOf("mp3", "mp2", "aac", "ogg", "oga", "opus", "webm", "wma", "mpc", "ac3", "vorbis")
        private val LOSSLESS_FORMATS = setOf("flac", "alac", "wav", "wave", "aiff", "aif", "aifc", "ape", "wv", "tta", "dsf", "dff", "wavpack")
        private val DSD_FORMATS = setOf("dsf", "dff")
        private val BITS_RATE = Regex("(\\d+)/(\\d+(?:\\.\\d+)?)")
        private val KBPS = Regex("(\\d+)k\\b")

        /**
         * The quality of a song. [transportKind]: "original" | "flac" | "aac-96" | "cadenza-96000" | "local" ("" before
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
            // the file's format as the start of an arrow: "FLAC → Cadenza 192 kHz", "FLAC → AAC 96"
            val from = sourceName(format, bitrate, bitDepth)?.let { "$it → " } ?: ""
            if (transportKind.startsWith("cadenza-")) {
                val rate = transportKind.removePrefix("cadenza-").toIntOrNull() ?: 0
                // the server's Cadenza renders 24-bit FLAC
                return CarQuality(Kind.CADENZA, if (rate > 0) "${from}Cadenza ${khz(rate)} kHz" else "${from}Cadenza")
            }
            if (phoneRate > 0) return CarQuality(Kind.CADENZA, "${from}Cadenza ${khz(phoneRate)} kHz")
            if (transportKind.startsWith("aac-")) {
                val kbps = transportKind.removePrefix("aac-").toIntOrNull() ?: 0
                return CarQuality(Kind.LOSSY, if (kbps > 0) "${from}AAC $kbps" else "${from}AAC")
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
            // named by what is inside
            val shown = if (f == "m4a" || f == "mp4") (if (lossless) "ALAC" else "AAC") else name
            if (!lossless) return CarQuality(Kind.LOSSY, if (bitrate > 0) "$shown $bitrate" else shown)
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

        /** A downloaded copy's label (OfflineIndex.qualityLabel): "FLAC 24/96", "AAC 256k" (shown "AAC 256"), "FLAC (converted)". */
        fun ofLabel(label: String): CarQuality? {
            val l = KBPS.replace(label.trim(), "$1")
            val lossyRate = KBPS.containsMatchIn(label)
            if (l.isEmpty()) return null
            BITS_RATE.find(l)?.let { m ->
                val bits = m.groupValues[1].toIntOrNull() ?: 0
                val khz = m.groupValues[2].toDoubleOrNull() ?: 0.0
                return CarQuality(if (bits > 16 || khz > 48.0) Kind.HIRES else Kind.LOSSLESS, l)
            }
            if (l.endsWith("(converted)")) return CarQuality(Kind.LOSSLESS, l)
            if (lossyRate) return CarQuality(Kind.LOSSY, l)
            val first = l.substringBefore(' ').lowercase(Locale.ROOT)
            return when (first) {
                in LOSSLESS_FORMATS -> CarQuality(Kind.LOSSLESS, l)
                in LOSSY_FORMATS -> CarQuality(Kind.LOSSY, l)
                else -> null
            }
        }

        /** The file's format for the start of an arrow ("FLAC", "ALAC", "AAC"); null when unknown. */
        private fun sourceName(format: String, bitrate: Int, bitDepth: Int): String? {
            val f = format.trim().lowercase(Locale.ROOT)
            if (f.isEmpty()) return null
            if (f == "m4a" || f == "mp4") return if (bitDepth > 0 || bitrate >= 500) "ALAC" else "AAC"
            return f.uppercase(Locale.ROOT)
        }

        /** 44100 → "44.1", 96000 → "96", 22050 → "22.05". */
        fun khz(hz: Int): String = String.format(Locale.US, "%.3f", hz / 1000.0).trimEnd('0').trimEnd('.')
    }
}
