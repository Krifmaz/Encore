package app.encore.android.playback.output

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioMixerAttributes
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.annotation.RequiresApi
import androidx.media3.common.C
import app.encore.android.data.prefs.Quality
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** A USB DAC that offers bit-perfect output (Android 14+): its name and the formats it takes, e.g. "24-bit / 96 kHz". */
data class UsbBitPerfect(val deviceId: Int, val deviceName: String, val formats: List<String>)

/**
 * Bit-perfect output to a USB DAC (PLAN §6.6, M6; Android 14+): watches USB audio devices come and go, lists the
 * DAC's bit-perfect mixer formats (`AudioManager.getSupportedMixerAttributes`) and, while [enabled], sets the one
 * matching each song ([choose], called by the effects processor before the AudioTrack is built) with
 * `setPreferredMixerAttributes`, so Android's mixer passes the samples through untouched. When nothing matches it
 * clears the preference and the song goes through the mixer as usual; [lastNote] says why.
 *
 * Below Android 14 it never finds a device and [state] stays null. Settings keeps [state] current without the
 * playback service through [refresh] and [watch].
 */
class UsbOutputManager(context: Context) {
    private val am = context.applicationContext.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
    private val main = Handler(Looper.getMainLooper())
    private var registered = false

    @Volatile private var device: AudioDeviceInfo? = null
    @Volatile private var preferred: Any? = null   // AudioMixerAttributes on API 34+

    /** PhonePrefs.bitPerfect: set by the playback service. */
    @Volatile var enabled: Boolean = false
        set(value) {
            field = value
            if (!value) clear()
        }

    /** The song that plays goes straight to the DAC (the last [choose] found a bit-perfect format). */
    @Volatile var active: Boolean = false
        private set

    /** Why the last song did not go bit-perfect ("" when it did, or when bit-perfect output is off). */
    @Volatile var lastNote: String = ""
        private set

    /** Main thread: the DAC came or went (the service then re-routes the player). */
    var onChange: (() -> Unit)? = null

    /** The DAC to route to while bit-perfect output is on and one is there. */
    val target: AudioDeviceInfo? get() = if (enabled) device else null

    private val callback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>?) = rescan()
        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>?) = rescan()
    }

    fun start() {
        if (Build.VERSION.SDK_INT < 34 || am == null || registered) return
        try {
            am.registerAudioDeviceCallback(callback, main)
            registered = true
        } catch (_: Exception) {}
        rescan()
    }

    fun stop() {
        if (registered) {
            try { am?.unregisterAudioDeviceCallback(callback) } catch (_: Exception) {}
            registered = false
        }
        clear()
        device = null
        // Settings still shows the DAC while nothing plays: keep the state as a fresh scan finds it.
        _state.value = if (Build.VERSION.SDK_INT >= 34) Api34.findDac(am)?.second else null
    }

    private fun rescan() {
        val before = device
        val found = if (Build.VERSION.SDK_INT >= 34) Api34.findDac(am) else null
        if (found == null) {
            if (before != null) clear()
            device = null
            _state.value = null
        } else {
            if (before != null && before.id != found.first.id) clear()
            device = found.first
            _state.value = found.second
        }
        if (before?.id != device?.id) onChange?.invoke()
    }

    /**
     * Picks and sets the DAC's bit-perfect format for a song at `rate` Hz with `channels` channels (playback thread,
     * before the AudioTrack is created). Upsampled songs need 24 or 32 bits (Cadenza's output is 24-bit); others
     * prefer 32, 24, then 16 bits (the song's 16-bit samples fit every one of them exactly). Returns the encoding to
     * hand over, or `C.ENCODING_INVALID` for Android's mixer.
     */
    fun choose(rate: Int, channels: Int, upsampled: Boolean): Int {
        val dac = device
        val manager = am
        val encoding = if (enabled && dac != null && manager != null && Build.VERSION.SDK_INT >= 34) {
            Api34.apply(manager, dac, rate, channels, upsampled) { attrs -> preferred = attrs }
        } else {
            active = false
            lastNote = ""
            return C.ENCODING_INVALID
        }
        active = encoding != C.ENCODING_INVALID
        if (!active) {
            clear()
            val khz = Quality.khz(rate)
            lastNote = if (upsampled) "The DAC has no 24-bit bit-perfect mode at $khz kHz, so Android's mixer plays it"
            else "The DAC has no bit-perfect mode at $khz kHz, so Android's mixer plays it"
        } else {
            lastNote = ""
        }
        return encoding
    }

    /** Drops the bit-perfect preference (unplug, switch off, a song the DAC can't take). */
    fun clear() {
        active = false
        val dac = device
        val manager = am
        if (Build.VERSION.SDK_INT >= 34) {
            if (preferred != null && dac != null && manager != null) Api34.clear(manager, dac)
        }
        preferred = null
    }

    @RequiresApi(34)
    private object Api34 {
        private val media: AudioAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .build()

        private val USB = setOf(AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET)

        private fun bitPerfect(am: AudioManager, d: AudioDeviceInfo): List<AudioMixerAttributes> = try {
            am.getSupportedMixerAttributes(d).filter { it.mixerBehavior == AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT }
        } catch (_: Exception) {
            emptyList()
        }

        fun findDac(am: AudioManager?): Pair<AudioDeviceInfo, UsbBitPerfect>? {
            if (am == null) return null
            val outs = try { am.getDevices(AudioManager.GET_DEVICES_OUTPUTS).toList() } catch (_: Exception) { emptyList() }
            for (d in outs) {
                if (d.type !in USB) continue
                val attrs = bitPerfect(am, d)
                if (attrs.isEmpty()) continue
                val formats = attrs.map { label(it.format) }.distinct()
                val name = d.productName?.toString()?.trim() ?: ""
                return d to UsbBitPerfect(d.id, name, formats)
            }
            return null
        }

        private fun bits(encoding: Int): Int = when (encoding) {
            AudioFormat.ENCODING_PCM_16BIT -> 16
            AudioFormat.ENCODING_PCM_24BIT_PACKED -> 24
            AudioFormat.ENCODING_PCM_32BIT -> 32
            AudioFormat.ENCODING_PCM_FLOAT -> 32
            else -> 0
        }

        private fun label(f: AudioFormat): String {
            val b = bits(f.encoding)
            val kind = if (f.encoding == AudioFormat.ENCODING_PCM_FLOAT) "32-bit float" else "$b-bit"
            return if (f.sampleRate > 0) "$kind / ${Quality.khz(f.sampleRate)} kHz" else kind
        }

        /** Sets the matching bit-perfect format and returns its Media3 encoding (`C.ENCODING_INVALID` when none). */
        fun apply(am: AudioManager, d: AudioDeviceInfo, rate: Int, channels: Int, upsampled: Boolean, set: (AudioMixerAttributes) -> Unit): Int {
            val mask = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
            if (channels > 2) return C.ENCODING_INVALID
            val order = if (upsampled) listOf(AudioFormat.ENCODING_PCM_32BIT, AudioFormat.ENCODING_PCM_24BIT_PACKED)
            else listOf(AudioFormat.ENCODING_PCM_32BIT, AudioFormat.ENCODING_PCM_24BIT_PACKED, AudioFormat.ENCODING_PCM_16BIT)
            val candidates = bitPerfect(am, d).filter { it.format.sampleRate == rate && it.format.channelMask == mask }
            for (enc in order) {
                val attrs = candidates.firstOrNull { it.format.encoding == enc } ?: continue
                val ok = try { am.setPreferredMixerAttributes(media, d, attrs) } catch (_: Exception) { false }
                if (!ok) continue
                set(attrs)
                return when (enc) {
                    AudioFormat.ENCODING_PCM_32BIT -> C.ENCODING_PCM_32BIT
                    AudioFormat.ENCODING_PCM_24BIT_PACKED -> C.ENCODING_PCM_24BIT
                    else -> C.ENCODING_PCM_16BIT
                }
            }
            return C.ENCODING_INVALID
        }

        fun clear(am: AudioManager, d: AudioDeviceInfo) {
            try { am.clearPreferredMixerAttributes(media, d) } catch (_: Exception) {}
        }
    }

    companion object {
        private val _state = MutableStateFlow<UsbBitPerfect?>(null)

        /** The USB DAC that offers bit-perfect output right now (null: none, or Android below 14). Read by Settings. */
        val state: StateFlow<UsbBitPerfect?> = _state.asStateFlow()

        /** Scans for a bit-perfect USB DAC now and updates [state] (no-op below Android 14). */
        fun refresh(context: Context) {
            if (Build.VERSION.SDK_INT < 34) return
            val am = context.applicationContext.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
            _state.value = Api34.findDac(am)?.second
        }

        /**
         * Keeps [state] current while the caller watches (Settings, with nothing playing): scans now and on every
         * audio device change until closed. A no-op handle below Android 14.
         */
        fun watch(context: Context): AutoCloseable {
            if (Build.VERSION.SDK_INT < 34) return AutoCloseable {}
            val app = context.applicationContext
            val am = app.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return AutoCloseable {}
            val cb = object : AudioDeviceCallback() {
                override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>?) = refresh(app)
                override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>?) = refresh(app)
            }
            try {
                am.registerAudioDeviceCallback(cb, Handler(Looper.getMainLooper()))
            } catch (_: Exception) {
                refresh(app)
                return AutoCloseable {}
            }
            refresh(app)
            return AutoCloseable { try { am.unregisterAudioDeviceCallback(cb) } catch (_: Exception) {} }
        }
    }
}
