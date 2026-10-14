package app.encore.android.playback

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build
import android.os.Bundle

/** What this phone does with the sound: the output it plays through and the rate the system mixer runs at. */
data class DeviceOutput(val rateHz: Int, val deviceName: String, val deviceType: String) {
    /** "USB DAC" / "Bluetooth" / "Wired" / "Speaker", with the product name when the device has one. */
    val label: String get() = if (deviceName.isNotEmpty() && deviceName != deviceType) "$deviceType ($deviceName)" else deviceType

    companion object {
        fun of(context: Context): DeviceOutput {
            val am = context.applicationContext.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
                ?: return DeviceOutput(0, "", "Speaker")
            val rate = am.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE)?.toIntOrNull() ?: 0
            val dev = activeDevice(am)
            return DeviceOutput(rate, dev?.productName?.toString()?.trim() ?: "", typeLabel(dev))
        }

        private fun activeDevice(am: AudioManager): AudioDeviceInfo? {
            if (Build.VERSION.SDK_INT >= 33) {
                try {
                    val attrs = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MUSIC).build()
                    val list = am.getAudioDevicesForAttributes(attrs)
                    if (list.isNotEmpty()) return list[0]
                } catch (_: Exception) {}
            }
            val outs = try { am.getDevices(AudioManager.GET_DEVICES_OUTPUTS).toList() } catch (_: Exception) { emptyList() }
            // the order Android itself prefers: USB, Bluetooth, wired, then the built-in speaker
            val order = listOf(
                AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET, AudioDeviceInfo.TYPE_USB_ACCESSORY,
                AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET,
                AudioDeviceInfo.TYPE_LINE_ANALOG, AudioDeviceInfo.TYPE_LINE_DIGITAL, AudioDeviceInfo.TYPE_HDMI,
                AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
            )
            for (t in order) outs.firstOrNull { it.type == t }?.let { return it }
            return outs.firstOrNull()
        }

        fun typeLabel(d: AudioDeviceInfo?): String = when (d?.type) {
            AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET, AudioDeviceInfo.TYPE_USB_ACCESSORY -> "USB DAC"
            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_BLUETOOTH_SCO -> "Bluetooth"
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_LINE_ANALOG, AudioDeviceInfo.TYPE_LINE_DIGITAL -> "Wired"
            AudioDeviceInfo.TYPE_HDMI, AudioDeviceInfo.TYPE_HDMI_ARC -> "HDMI"
            null -> "Speaker"
            else -> if (d.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER || d.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE) "Speaker" else "Audio output"
        }
    }
}

/**
 * What the playback service tells the UI about the song that plays, beyond what the Media3 player carries:
 * the quality badge, the transport (what Encore sent) and the position offset of a server-made stream. It travels
 * as the session's extras (MediaSession.setSessionExtras → MediaController.Listener.onExtrasChanged).
 */
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
    /** what the effects engine does (DspEngine.signalPath), "" when the effects are off */
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
    fun toBundle(): Bundle = Bundle().apply {
        putLong(K_ID, trackId)
        putString(K_BADGE, qualityBadge)
        putString(K_TITLE, qualityTitle)
        putString(K_KIND, transportKind)
        putString(K_TEXT, transportText)
        putLong(K_OFFSET, offsetMs)
        putBoolean(K_SERVER, serverStream)
        putDouble(K_KBPS, connectionKbps)
        putString(K_ENGINE, effectsEngine)
        putInt(K_RATE, processorRate)
        putBoolean(K_PHONE, phoneCadenza)
        putInt(K_PHONE_RATE, phoneRate)
        putString(K_PHONE_FILTER, phoneFilterLabel)
        putString(K_PHONE_DITHER, phoneDitherLabel)
        putBoolean(K_USB_BP, usbBitPerfect)
        putString(K_USB_DEVICE, usbDevice)
        putString(K_USB_NOTE, usbNote)
    }

    companion object {
        const val K_ID = "encore.track_id"
        const val K_BADGE = "encore.quality_badge"
        const val K_TITLE = "encore.quality_title"
        const val K_KIND = "encore.transport_kind"
        const val K_TEXT = "encore.transport_text"
        const val K_OFFSET = "encore.offset_ms"
        const val K_SERVER = "encore.server_stream"
        const val K_KBPS = "encore.connection_kbps"
        const val K_ENGINE = "encore.effects_engine"
        const val K_RATE = "encore.processor_rate"
        const val K_PHONE = "encore.phone_cadenza"
        const val K_PHONE_RATE = "encore.phone_rate"
        const val K_PHONE_FILTER = "encore.phone_filter"
        const val K_PHONE_DITHER = "encore.phone_dither"
        const val K_USB_BP = "encore.usb_bit_perfect"
        const val K_USB_DEVICE = "encore.usb_device"
        const val K_USB_NOTE = "encore.usb_note"

        fun from(b: Bundle?): StreamInfo? {
            if (b == null || !b.containsKey(K_BADGE)) return null
            return StreamInfo(
                trackId = b.getLong(K_ID, 0L),
                qualityBadge = b.getString(K_BADGE) ?: "",
                qualityTitle = b.getString(K_TITLE) ?: "",
                transportKind = b.getString(K_KIND) ?: "",
                transportText = b.getString(K_TEXT) ?: "",
                offsetMs = b.getLong(K_OFFSET, 0L),
                serverStream = b.getBoolean(K_SERVER, false),
                connectionKbps = b.getDouble(K_KBPS, 0.0),
                effectsEngine = b.getString(K_ENGINE) ?: "",
                processorRate = b.getInt(K_RATE, 0),
                phoneCadenza = b.getBoolean(K_PHONE, false),
                phoneRate = b.getInt(K_PHONE_RATE, 0),
                phoneFilterLabel = b.getString(K_PHONE_FILTER) ?: "",
                phoneDitherLabel = b.getString(K_PHONE_DITHER) ?: "",
                usbBitPerfect = b.getBoolean(K_USB_BP, false),
                usbDevice = b.getString(K_USB_DEVICE) ?: "",
                usbNote = b.getString(K_USB_NOTE) ?: "",
            )
        }
    }
}
