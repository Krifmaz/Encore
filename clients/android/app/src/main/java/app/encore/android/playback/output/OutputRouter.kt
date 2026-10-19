package app.encore.android.playback.output

import android.content.Context
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import app.encore.android.cadenza.CadenzaCatalog
import app.encore.android.cadenza.CadenzaEngine
import app.encore.android.cadenza.EngineSetup
import app.encore.android.data.prefs.PhonePrefs
import app.encore.android.playback.CurrentStream
import app.encore.android.playback.StreamResolver

/**
 * What happens to the sound after the decoder on this phone (Settings → Playback → "Cadenza engine · this phone",
 * [PhonePrefs], device-only): Cadenza upsampling inside the app and bit-perfect output to a USB DAC
 * ([UsbOutputManager], Android 14+). The playback service owns one: the effects processor asks it what to do with
 * each song ([setupFor], [directOutput]), and it re-routes the player and restarts the song at the same spot when a
 * setting or the DAC changes. Main thread, except [setupFor] and [directOutput] (playback thread).
 */
class OutputRouter(
    private val context: Context,
    private val resolver: StreamResolver,
    initial: PhonePrefs,
    /** the service is gone: callbacks that arrive late do nothing */
    private val released: () -> Boolean,
    /** the session's extras say what changed (the Signal path) */
    private val publish: () -> Unit,
) {
    val usb = UsbOutputManager(context)

    /** This phone's own Cadenza / USB settings (PrefsRepository.phone). */
    var phone: PhonePrefs = initial
        private set

    /** the rate phone Cadenza upsamples to (0 = off); read on the playback thread at every configure */
    @Volatile var phoneOutRate = 0
        private set

    /** this phone's mixer rate, for PhonePrefs.rate "device" (and the server's dsp_rate "device") */
    @Volatile var deviceRate = 0
        private set

    @Volatile private var filterId = ""
    @Volatile private var ditherId = ""

    private var player: ExoPlayer? = null
    private var current: CurrentStream? = null

    init {
        deviceRate = DeviceOutput.of(context).rateHz
        resolver.deviceRate = deviceRate
        applyEngine(initial)
    }

    /** The player exists: watch the USB DAC and route to it. */
    fun start(player: ExoPlayer, current: CurrentStream) {
        this.player = player
        this.current = current
        usb.onChange = { if (!released()) usbChanged() }
        usb.start()
        route()
    }

    fun stop() {
        usb.onChange = null
        usb.stop()
    }

    /**
     * Per song, on the playback thread, just before the engine is configured: the server's render wins over the
     * phone's Cadenza (a song the server upsampled passes through at its rate).
     */
    fun setupFor(inRate: Int): EngineSetup =
        EngineSetup(if (resolver.serverRenderAt(inRate)) 0 else phoneOutRate, filterId, ditherId)

    /** Bit-perfect USB: the DAC mixer's encoding for this format, or `C.ENCODING_INVALID` (UsbOutputManager.choose). */
    fun directOutput(rate: Int, channels: Int, upsampled: Boolean): Int = usb.choose(rate, channels, upsampled)

    /** Bit-perfect output is on and a DAC is there: song transitions stay off (they need Android's mixer). */
    val bitPerfectRoute: Boolean get() = phone.bitPerfect && usb.target != null

    /** The song that plays goes to the DAC bit-perfect right now. */
    val bitPerfectNow: Boolean get() = phone.bitPerfect && usb.target != null && usb.active

    /** The DAC's name while one is the target ("" = none). */
    val usbDeviceName: String get() = if (usb.target != null) (UsbOutputManager.state.value?.deviceName ?: "") else ""

    /** Why bit-perfect output is on but this song went through the mixer ("" when it did not, or when it is off). */
    val usbNote: String get() = if (phone.bitPerfect && usb.target != null && !bitPerfectNow) usb.lastNote else ""

    /** The phone engine's filter / dither as the Signal path names them. */
    fun filterLabel(id: String): String = CadenzaCatalog.phone.filterLabel(id)

    fun ditherLabel(id: String): String = CadenzaCatalog.phone.ditherLabel(id)

    /** The engine's static settings (read at the next configure). */
    private fun applyEngine(p: PhonePrefs) {
        filterId = p.filter
        ditherId = p.dither
        phoneOutRate = if (CadenzaEngine.available) p.outputRateHz(deviceRate) else 0
        usb.enabled = p.bitPerfect
    }

    /**
     * New phone settings: they apply at the next song, except a new output rate / filter / dither or bit-perfect
     * switch while a song plays, which restarts it at the same spot so the sink is set up again.
     */
    fun phoneChanged(p: PhonePrefs) {
        val before = phone
        val rateBefore = phoneOutRate
        phone = p
        applyEngine(p)
        val engineChanged = phoneOutRate != rateBefore || (phoneOutRate > 0 && (before.filter != p.filter || before.dither != p.dither))
        val bitPerfectChanged = before.bitPerfect != p.bitPerfect
        if (bitPerfectChanged) route()
        // bit-perfect on or off only changes the path when a DAC is plugged in
        if (engineChanged || (bitPerfectChanged && UsbOutputManager.state.value != null)) restartForOutput()
        publish()
    }

    /** The DAC came or went: this phone's mixer rate may have changed too; re-route and restart if bit-perfect is on. */
    private fun usbChanged() {
        deviceRate = DeviceOutput.of(context).rateHz
        resolver.deviceRate = deviceRate
        val rateBefore = phoneOutRate
        applyEngine(phone)
        route()
        if (phone.bitPerfect || phoneOutRate != rateBefore) restartForOutput()
        publish()
    }

    /** ExoPlayer plays to the DAC while bit-perfect output is on (null: Android's choice). */
    private fun route() {
        val p = player ?: return
        try { p.setPreferredAudioDevice(usb.target) } catch (_: Exception) {}
        if (usb.target == null) usb.clear()
    }

    /** The song that plays again from where it is, so the audio sink is configured anew (a new rate or output path). */
    private fun restartForOutput() {
        val player = player ?: return
        val current = current ?: return
        if (player.currentMediaItem == null || player.playbackState == Player.STATE_IDLE) return
        val wasPlaying = player.playWhenReady
        val t = current.track()
        val v = current.variant()
        if (t != null && v != null && v.serverStream) {
            // a stream the server makes as it goes: ask for it again at the spot
            val at = current.absPositionMs() / 1000.0
            player.stop()
            current.reopen(t, rung = null, dsp = null, offsetSec = if (at > 0.5) at else 0.0, prepareFirst = true)
            player.playWhenReady = wasPlaying
            return
        }
        val index = player.currentMediaItemIndex
        val pos = player.currentPosition.coerceAtLeast(0L)
        player.stop()
        player.prepare()
        player.seekTo(index, pos)
        player.playWhenReady = wasPlaying
    }
}
