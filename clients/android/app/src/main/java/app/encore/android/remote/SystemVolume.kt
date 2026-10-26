package app.encore.android.remote

import android.content.Context
import android.database.ContentObserver
import android.media.AudioManager
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import kotlin.math.roundToInt

/**
 * This phone's (or TV's) own media volume – the one the volume keys change – for another device controlling it
 * as a remote: [level] is what the remote snapshot reports as `system_volume`, [set] runs the `system_volume`
 * command, and [watch] tells the service when someone changes it here, so the other devices see it at once.
 *
 * A device whose volume is fixed (many TVs on HDMI, where the TV or receiver owns the volume) reports nothing,
 * and the remotes leave their "Device" slider out.
 */
class SystemVolume(context: Context) {
    /** the system's audio service (null on a device without one) */
    private val audio = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager

    /** the content resolver the volume settings live in, for [watch] */
    private val resolver = context.contentResolver

    /** the observer [watch] registered, removed by [stop] */
    private var observer: ContentObserver? = null

    /** The media volume, 0..1 (0 while muted), or null when this device cannot change it. */
    fun level(): Double? {
        val a = audio ?: return null
        if (a.isVolumeFixed) return null
        if (a.isStreamMute(AudioManager.STREAM_MUSIC)) return 0.0
        return fraction(a.getStreamVolume(AudioManager.STREAM_MUSIC), a.getStreamMaxVolume(AudioManager.STREAM_MUSIC))
    }

    /**
     * Set the media volume to [value] (0..1, the nearest step). No volume panel is shown: the person changing it
     * is at the other device. A muted stream is unmuted. Do Not Disturb can refuse a change; that is left as it is.
     */
    fun set(value: Double) {
        val a = audio ?: return
        if (a.isVolumeFixed || value.isNaN()) return
        val steps = toSteps(value, a.getStreamMaxVolume(AudioManager.STREAM_MUSIC))
        try {
            // a level set is a level heard: unmute first (a muted stream keeps its level silently)
            if (steps > 0 && a.isStreamMute(AudioManager.STREAM_MUSIC)) a.adjustStreamVolume(AudioManager.STREAM_MUSIC, AudioManager.ADJUST_UNMUTE, 0)
            a.setStreamVolume(AudioManager.STREAM_MUSIC, steps, 0)
        } catch (_: SecurityException) {
        }
    }

    /** Call [changed] on the main thread whenever a volume changes on this device, until [stop]. */
    fun watch(changed: () -> Unit) {
        stop()
        val o = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) = changed()
        }
        // the stream volumes are kept in Settings.System: a change there is a volume key, a slider or a command
        resolver.registerContentObserver(Settings.System.CONTENT_URI, true, o)
        observer = o
    }

    /** Stop the [watch]. */
    fun stop() {
        observer?.let { resolver.unregisterContentObserver(it) }
        observer = null
    }

    companion object {
        /** A volume step out of [max] as 0..1 (0 when the stream has no steps). */
        fun fraction(step: Int, max: Int): Double = if (max <= 0) 0.0 else (step.toDouble() / max).coerceIn(0.0, 1.0)

        /** A 0..1 level as the nearest of the stream's [max] steps. */
        fun toSteps(value: Double, max: Int): Int = (value.coerceIn(0.0, 1.0) * max).roundToInt().coerceIn(0, max.coerceAtLeast(0))
    }
}
