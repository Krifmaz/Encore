package app.encore.android.playback

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * The sound effects (EQ, crossfeed, balance, mono) as a Media3 audio processor in ExoPlayer's audio sink, where
 * every decoded song passes through. It converts the sink's 16-bit PCM to float, hands it to a [DspEngine]
 * (Cadenza when available, Kotlin otherwise) and writes the result back as 16-bit PCM with TPDF dither.
 *
 * The processor stays in the chain once configured (like the web player's Web Audio graph) and is a byte-for-byte
 * pass-through while the effects are off, so switching them on takes effect right away.
 *
 * Runs on ExoPlayer's playback thread; the UI only sets [params].
 */
class EffectsAudioProcessor(private val engine: DspEngine) : BaseAudioProcessor() {
    @Volatile private var pendingParams: String = ""
    private var appliedParams: String = ""
    @Volatile var bypass: Boolean = true
        private set

    private var channels = 2
    private var inRate = 48000
    private var outRate = 48000
    private var floats = FloatArray(0)
    private var seed = 0x2545F491L

    /** The engine's output sample rate as configured for the current stream (0 before the first song). */
    @Volatile var configuredOutputRate: Int = 0
        private set

    /** The effects as JSON (PlaybackPrefs.engineParams). `bypass` says whether anything is applied. */
    var params: String
        get() = pendingParams
        set(value) {
            pendingParams = value
            bypass = !value.contains("\"fx_on\":true")
        }

    override fun onConfigure(inputAudioFormat: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
        if (inputAudioFormat.encoding != C.ENCODING_PCM_16BIT) throw AudioProcessor.UnhandledAudioFormatException(inputAudioFormat)
        channels = inputAudioFormat.channelCount
        inRate = inputAudioFormat.sampleRate
        outRate = try { engine.configure(inRate, channels) } catch (_: Throwable) { inRate }
        if (outRate <= 0) outRate = inRate
        configuredOutputRate = outRate
        appliedParams = ""
        return AudioProcessor.AudioFormat(outRate, channels, C.ENCODING_PCM_16BIT)
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        val p = pendingParams
        if (p != appliedParams) {
            appliedParams = p
            try { engine.setParams(p) } catch (_: Throwable) {}
        }
        val bytes = inputBuffer.remaining()
        if (bytes == 0) return
        if (bypass && outRate == inRate) {
            // untouched: the same bytes go out
            val out = replaceOutputBuffer(bytes)
            out.put(inputBuffer)
            out.flip()
            return
        }
        val frames = bytes / (2 * channels)
        val n = frames * channels
        if (floats.size < n) floats = FloatArray(n)
        val src = inputBuffer.order(ByteOrder.LITTLE_ENDIAN)
        for (i in 0 until n) floats[i] = src.getShort().toInt() / 32768f
        // anything left over (a partial frame) is dropped, as the sink never sends one
        inputBuffer.position(inputBuffer.limit())
        val processed = try { engine.process(floats, frames) } catch (_: Throwable) { floats }
        val outSamples = if (processed === floats) n else processed.size
        writeFloats(processed, outSamples)
    }

    override fun onQueueEndOfStream() {
        val tail = try { engine.flush() } catch (_: Throwable) { FloatArray(0) }
        if (tail.isNotEmpty()) writeFloats(tail, tail.size)
    }

    private fun writeFloats(data: FloatArray, samples: Int) {
        val out = replaceOutputBuffer(samples * 2).order(ByteOrder.LITTLE_ENDIAN)
        for (i in 0 until samples) {
            // TPDF dither (±1 LSB triangular), then clip
            val d = (nextRand() + nextRand()) / 32768f
            val v = data[i] * 32768f + d
            val s = if (v > 32767f) 32767 else if (v < -32768f) -32768 else Math.round(v)
            out.putShort(s.toShort())
        }
        out.flip()
    }

    /** A cheap uniform random in [-0.5, 0.5). */
    private fun nextRand(): Float {
        seed = seed * 6364136223846793005L + 1442695040888963407L
        return ((seed ushr 40).toInt() and 0xFFFF) / 65536f - 0.5f
    }

    override fun onFlush() {
        try { engine.flush() } catch (_: Throwable) {}
    }

    override fun onReset() {
        try { engine.flush() } catch (_: Throwable) {}
        floats = FloatArray(0)
    }

    fun release() {
        try { engine.release() } catch (_: Throwable) {}
    }

    /** What the engine says it does (for the Signal path). */
    fun engineSignalPath(): String = try { engine.signalPath() } catch (_: Throwable) { "" }
}
