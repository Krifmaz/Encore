package app.encore.android.playback.output

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build

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
