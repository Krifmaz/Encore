package app.encore.android.data.prefs

import app.encore.android.core.bool
import app.encore.android.core.stringOnly
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * Settings that describe this phone's own hardware (PLAN §6.3 / §6.6, M6): Cadenza upsampling inside the app and
 * bit-perfect output to a USB DAC. They stay on this device (SharedPreferences key "phone_cadenza") and are not
 * part of the account's `prefs.playback`, so another phone or the web never gets them.
 *
 * cadenza: upsample on this phone with Cadenza · filter / dither: ids from `CadenzaEngine.catalog()` ("" = the
 * engine's defaults, sinc-l-64k and TPDF) · rate: "device" (this phone's mixer rate) or a fixed rate in Hz ·
 * bitPerfect: send directly to the USB DAC (Android 14+, only when one offers it).
 */
data class PhonePrefs(
    val cadenza: Boolean = false,
    val filter: String = "",
    val dither: String = "",
    val rate: String = "device",
    val bitPerfect: Boolean = false,
) {
    /** The rate Cadenza upsamples to on this phone: 0 when it is off, `deviceRate` for "device", else the number. */
    fun outputRateHz(deviceRate: Int): Int {
        if (!cadenza) return 0
        if (rate == "device") return deviceRate.coerceAtLeast(0)
        return rate.toIntOrNull()?.takeIf { it > 0 } ?: deviceRate.coerceAtLeast(0)
    }

    fun toJson(): JsonObject = buildJsonObject {
        put("cadenza", cadenza)
        put("filter", filter)
        put("dither", dither)
        put("rate", rate)
        put("bit_perfect", bitPerfect)
    }

    companion object {
        /** The fixed rates on offer besides "device". */
        val RATES = listOf(44100, 48000, 88200, 96000, 176400, 192000)

        /** Read back what [toJson] wrote; anything missing or unknown keeps its default. */
        fun from(o: JsonObject?): PhonePrefs {
            if (o == null) return PhonePrefs()
            val rate = o.stringOnly("rate").takeIf { it == "device" || RATES.contains(it.toIntOrNull() ?: 0) } ?: "device"
            return PhonePrefs(
                cadenza = o.bool("cadenza"),
                filter = o.stringOnly("filter"),
                dither = o.stringOnly("dither"),
                rate = rate,
                bitPerfect = o.bool("bit_perfect"),
            )
        }
    }
}
