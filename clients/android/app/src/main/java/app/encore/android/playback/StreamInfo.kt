package app.encore.android.playback

import android.os.Bundle
import app.encore.android.core.EncoreJson
import kotlinx.serialization.Serializable

/**
 * What the playback service tells the UI about the song that plays, beyond what the Media3 player carries:
 * the quality badge, the transport (what Encore sent), the position offset of a server-made stream and what this
 * phone does with the sound. It travels as one JSON string in the session's extras ([KEY]; MediaSession
 * .setSessionExtras → MediaController.Listener.onExtrasChanged).
 */
@Serializable
data class StreamInfo(
    val trackId: Long = 0L,
    val qualityBadge: String = "",
    val qualityTitle: String = "",
    /** "original" | "flac" | "aac-96" | "cadenza-96000" */
    val transportKind: String = "",
    /** the "Encore → this device" row of the signal path */
    val transportText: String = "",
    /** the position a server-made stream started at (its clock starts there, the player's at 0) */
    val offsetMs: Long = 0L,
    /** a stream the server makes as it goes: seeks go through the service (`t=`), not the player */
    val serverStream: Boolean = false,
    /** the bandwidth estimate, kbit/s (0 = none yet) */
    val connectionKbps: Double = 0.0,
    /** what the effects engine does (CadenzaEngine.signalPath), "" when the effects are off */
    val effectsEngine: String = "",
    /** the rate the effects processor outputs (0 before the first song) */
    val processorRate: Int = 0,
    /** Cadenza on this phone upsampled this song */
    val phoneCadenza: Boolean = false,
    /** the rate phone Cadenza upsampled to, Hz (0 when it did not) */
    val phoneRate: Int = 0,
    /** the phone engine's upsampling filter, e.g. "Sinc L 64k" ("" when it did not upsample) */
    val phoneFilterLabel: String = "",
    /** the phone engine's dither, e.g. "TPDF" ("" when it did not upsample) */
    val phoneDitherLabel: String = "",
    /** the song goes to the USB DAC through Android's bit-perfect mixer path */
    val usbBitPerfect: Boolean = false,
    /** the USB DAC's product name ("" = none) */
    val usbDevice: String = "",
    /** why bit-perfect output is on but this song went through the mixer ("" = it did not, or it is off) */
    val usbNote: String = "",
) {
    fun toBundle(): Bundle = Bundle().apply { putString(KEY, EncoreJson.encodeToString(serializer(), this@StreamInfo)) }

    companion object {
        /** The session extra holding the JSON. */
        const val KEY = "encore.stream_info"

        /** What [toBundle] wrote; null for extras without it. */
        fun from(b: Bundle?): StreamInfo? {
            val text = b?.getString(KEY) ?: return null
            return try { EncoreJson.decodeFromString(serializer(), text) } catch (_: Exception) { null }
        }
    }
}
