package app.encore.android.playback

import android.os.Build
import androidx.media3.common.C
import androidx.media3.common.PlaybackParameters
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.AudioProcessorChain
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
 * When Cadenza upsamples on the phone (the engine's output rate differs from the song's) its output is already
 * dithered to 24 bits by Cadenza, so it leaves as 32-bit integer PCM (Android 12+; float before that, which carries
 * 24 bits exactly). With bit-perfect USB output on, [directOutput] picks the DAC mixer's encoding (16, 24 or 32-bit)
 * for every song. The 16-bit path with TPDF dither is only used when neither applies.
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

    private var outEncoding = C.ENCODING_PCM_16BIT

    /** The engine's output sample rate as configured for the current stream (0 before the first song). */
    @Volatile var configuredOutputRate: Int = 0
        private set

    /** The song's own rate as the sink hands it over (0 before the first song). */
    @Volatile var configuredInputRate: Int = 0
        private set

    /** The PCM encoding this processor hands to the sink for the current stream (a `C.ENCODING_PCM_*`). */
    @Volatile var configuredEncoding: Int = C.ENCODING_PCM_16BIT
        private set

    /**
     * Bit-perfect USB output: asked at every configure (on the playback thread) with the output rate, the channel
     * count and whether Cadenza upsampled; returns the encoding the DAC's bit-perfect mixer takes at that format
     * (`C.ENCODING_PCM_16BIT`, `_24BIT` or `_32BIT`, after setting it up), or `C.ENCODING_INVALID` for Android's mixer.
     * null while bit-perfect output is off.
     */
    @Volatile var directOutput: ((rate: Int, channels: Int, upsampled: Boolean) -> Int)? = null

    /**
     * Called with the song's rate just before the engine is configured for it (the service sets the phone's Cadenza
     * rate). It returns the filter and dither ids the engine will use, kept as [configuredFilterId] /
     * [configuredDitherId] so the Signal path names what this song really went through.
     */
    @Volatile var beforeConfigure: ((inRate: Int) -> Pair<String, String>)? = null

    /** The phone Cadenza filter id the current stream was configured with ("" = Cadenza's default). */
    @Volatile var configuredFilterId: String = ""
        private set

    /** The phone Cadenza dither id the current stream was configured with ("" = Cadenza's default). */
    @Volatile var configuredDitherId: String = ""
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
        val ids = try { beforeConfigure?.invoke(inRate) } catch (_: Throwable) { null }
        configuredFilterId = ids?.first ?: ""
        configuredDitherId = ids?.second ?: ""
        outRate = try { engine.configure(inRate, channels) } catch (_: Throwable) { inRate }
        if (outRate <= 0) outRate = inRate
        val upsampled = outRate != inRate
        val direct = directOutput?.let { pick -> try { pick(outRate, channels, upsampled) } catch (_: Throwable) { C.ENCODING_INVALID } }
            ?: C.ENCODING_INVALID
        outEncoding = when {
            direct == C.ENCODING_PCM_32BIT || direct == C.ENCODING_PCM_24BIT -> direct
            // Cadenza's 24-bit output never goes down to 16 bits
            direct == C.ENCODING_PCM_16BIT && !upsampled -> direct
            upsampled -> if (Build.VERSION.SDK_INT >= 31) C.ENCODING_PCM_32BIT else C.ENCODING_PCM_FLOAT
            else -> C.ENCODING_PCM_16BIT
        }
        configuredInputRate = inRate
        configuredOutputRate = outRate
        configuredEncoding = outEncoding
        appliedParams = ""
        return AudioProcessor.AudioFormat(outRate, channels, outEncoding)
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
            if (outEncoding == C.ENCODING_PCM_16BIT) {
                // untouched: the same bytes go out
                val out = replaceOutputBuffer(bytes)
                out.put(inputBuffer)
                out.flip()
            } else {
                widenShorts(inputBuffer, bytes / 2)
            }
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
        if (processed === floats && outRate != inRate) return   // unprocessed input can't go out at another rate: dropped
        val outSamples = if (processed === floats) n else processed.size
        writeFloats(processed, outSamples)
    }

    override fun onQueueEndOfStream() {
        val tail = try { engine.flush() } catch (_: Throwable) { FloatArray(0) }
        if (tail.isNotEmpty()) writeFloats(tail, tail.size)
    }

    /** The song's 16-bit samples, unchanged, in a wider container (bit-perfect output, effects off). */
    private fun widenShorts(input: ByteBuffer, samples: Int) {
        val src = input.order(ByteOrder.LITTLE_ENDIAN)
        val out = replaceOutputBuffer(samples * bytesPerSample()).order(ByteOrder.LITTLE_ENDIAN)
        for (i in 0 until samples) {
            val s = src.getShort().toInt()
            when (outEncoding) {
                C.ENCODING_PCM_32BIT -> out.putInt(s shl 16)
                C.ENCODING_PCM_24BIT -> put24(out, s shl 8)
                else -> out.putFloat(s / 32768f)
            }
        }
        input.position(input.limit())
        out.flip()
    }

    private fun bytesPerSample(): Int = when (outEncoding) {
        C.ENCODING_PCM_16BIT -> 2
        C.ENCODING_PCM_24BIT -> 3
        else -> 4
    }

    /** One 24-bit little-endian sample. */
    private fun put24(out: ByteBuffer, v: Int) {
        out.put((v and 0xFF).toByte())
        out.put(((v shr 8) and 0xFF).toByte())
        out.put(((v shr 16) and 0xFF).toByte())
    }

    private fun writeFloats(data: FloatArray, samples: Int) {
        if (outEncoding != C.ENCODING_PCM_16BIT) {
            writeWide(data, samples)
            return
        }
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

    /**
     * 24-bit precision out: Cadenza's upsampled output is already dithered to 24-bit steps, so it is only scaled
     * (exactly); the effects alone at the song's rate are rounded to 24 bits.
     */
    private fun writeWide(data: FloatArray, samples: Int) {
        val out = replaceOutputBuffer(samples * bytesPerSample()).order(ByteOrder.LITTLE_ENDIAN)
        for (i in 0 until samples) {
            val f = data[i]
            if (outEncoding == C.ENCODING_PCM_FLOAT) {
                out.putFloat(if (f > 1f) 1f else if (f < -1f) -1f else f)
                continue
            }
            val v = f * 8388608f
            val s = if (v >= 8388607f) 8388607 else if (v <= -8388608f) -8388608 else Math.round(v)
            if (outEncoding == C.ENCODING_PCM_32BIT) out.putInt(s shl 8) else put24(out, s)
        }
        out.flip()
    }

    /** A cheap uniform random in [-0.5, 0.5). */
    private fun nextRand(): Float {
        seed = seed * 6364136223846793005L + 1442695040888963407L
        return ((seed ushr 40).toInt() and 0xFFFF) / 65536f - 0.5f
    }

    override fun onFlush(streamMetadata: AudioProcessor.StreamMetadata) {
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

/**
 * The sink's processor chain: only [EffectsAudioProcessor]. Media3's default chain adds silence skipping and Sonic
 * (speed), which take 16-bit PCM only and would refuse the 24 / 32-bit output of phone Cadenza or bit-perfect USB;
 * the app never changes speed or skips silence, so they are left out (speed stays 1).
 */
class EffectsChain(private val effects: EffectsAudioProcessor) : AudioProcessorChain {
    private val processors: Array<AudioProcessor> = arrayOf<AudioProcessor>(effects)

    override fun getAudioProcessors(): Array<AudioProcessor> = processors

    override fun applyPlaybackParameters(playbackParameters: PlaybackParameters): PlaybackParameters = PlaybackParameters.DEFAULT

    override fun applySkipSilenceEnabled(skipSilenceEnabled: Boolean): Boolean = false

    override fun getMediaDuration(playoutDuration: Long): Long = playoutDuration

    override fun getSkippedOutputFrameCount(): Long = 0L
}
