package app.encore.android.playback.output

import android.os.Build
import androidx.media3.common.C
import androidx.media3.common.PlaybackParameters
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.AudioProcessorChain
import androidx.media3.common.audio.BaseAudioProcessor
import app.encore.android.cadenza.CadenzaEngine
import app.encore.android.cadenza.EngineSetup
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer

/**
 * The sound effects (EQ, crossfeed, balance, mono) as a Media3 audio processor in ExoPlayer's audio sink, where
 * every decoded song passes through. It converts the sink's 16-bit PCM to float in the engine's direct buffer,
 * has [CadenzaEngine] process it there (no copy across JNI) and writes the result back as 16-bit PCM with TPDF
 * dither. Without the Cadenza library in the APK the sound passes through untouched.
 *
 * The processor stays in the chain once configured (like the web player's Web Audio graph) and is a byte-for-byte
 * pass-through while the effects and volume normalization are off, so switching them on takes effect right away.
 *
 * Volume normalization (Settings → Playback → Sound) is the first stage, as the web's GainNode before its effects:
 * each song's gain ([gainAtConfigure], asked when the sink is configured for the song) multiplies the samples as they
 * go into Cadenza's input buffer, so it works with the effects off and in a build without the Cadenza library. A new
 * level in the middle of a song (a setting changed) is reached with a ramp over one buffer, so it doesn't click.
 * Songs that go bit-perfect to a USB DAC ([directOutput]) are never normalized.
 *
 * When Cadenza upsamples on the phone (the engine's output rate differs from the song's) its output is already
 * dithered to 24 bits by Cadenza, so it leaves as 32-bit integer PCM (Android 12+; float before that, which carries
 * 24 bits exactly). With bit-perfect USB output on, [directOutput] picks the DAC mixer's encoding (16, 24 or 32-bit)
 * for every song. The 16-bit path with TPDF dither is only used when neither applies.
 *
 * Runs on ExoPlayer's playback thread; the UI only sets [params].
 */
class EffectsAudioProcessor(private val engine: CadenzaEngine) : BaseAudioProcessor() {
    @Volatile private var pendingParams: String = ""
    private var appliedParams: String = ""
    @Volatile var bypass: Boolean = true
        private set

    private var channels = 2
    private var inRate = 48000
    private var outRate = 48000
    /** the engine was built for this stream (false without the library: the sound passes through) */
    private var engineReady = false
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
     * Called with the song's rate just before the engine is configured for it: what the engine does besides the
     * effects (the phone's Cadenza upsampling, OutputRouter.setupFor). Its filter and dither ids are kept as
     * [configuredFilterId] / [configuredDitherId] so the Signal path names what this song really went through.
     * null (the transitions' second player): effects only.
     */
    @Volatile var beforeConfigure: ((inRate: Int) -> EngineSetup)? = null

    /** The phone Cadenza filter id the current stream was configured with ("" = Cadenza's default). */
    @Volatile var configuredFilterId: String = ""
        private set

    /** The phone Cadenza dither id the current stream was configured with ("" = Cadenza's default). */
    @Volatile var configuredDitherId: String = ""
        private set

    /**
     * Volume normalization: the level the samples are played at (linear, 1 = untouched). Set from any thread; a
     * change mid-song is ramped over the next buffer. Ignored while the song goes bit-perfect to a USB DAC.
     */
    @Volatile var gain: Float = 1f

    /**
     * Asked at every configure (playback thread) for the gain of the song the sink is being configured for (linear;
     * the playback service's volume normalization). null (the transitions' second player): [gain] is set directly.
     */
    @Volatile var gainAtConfigure: (() -> Float)? = null

    /** The current stream goes bit-perfect to a USB DAC: no normalization (set at every configure). */
    @Volatile var configuredBitPerfect: Boolean = false
        private set

    /** the gain the last buffer ended at (playback thread) */
    private var appliedGain = 1f

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
        val setup = (try { beforeConfigure?.invoke(inRate) } catch (_: Throwable) { null }) ?: EngineSetup()
        configuredFilterId = setup.filterId
        configuredDitherId = setup.ditherId
        engineReady = false
        outRate = try { engine.configure(inRate, channels, setup).also { engineReady = true } } catch (_: Throwable) { inRate }
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
        // a new song: its own level from its first sample (no ramp); none at all on the bit-perfect path
        val bitPerfect = direct != C.ENCODING_INVALID
        configuredBitPerfect = bitPerfect
        val g = (try { gainAtConfigure?.invoke() } catch (_: Throwable) { null }) ?: gain
        gain = if (g.isNaN() || g <= 0f) 1f else g
        appliedGain = if (bitPerfect) 1f else gain
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
        val target = if (configuredBitPerfect) 1f else gain
        val from = appliedGain
        // nothing for the engine to do: effects off and no upsampling, or no engine (a build without the library)
        val plain = (bypass || !engineReady) && outRate == inRate
        // ... and no normalization either: the song as it is
        if (plain && from == 1f && target == 1f) {
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
        // the samples go straight into the engine's direct buffer (absolute puts, nothing copied across JNI), with the
        // normalization gain on the way in (the first stage, before the preamp and EQ)
        val floats = engine.input(n)
        val src = inputBuffer.order(ByteOrder.LITTLE_ENDIAN)
        if (from == target) {
            val k = target / 32768f
            for (i in 0 until n) floats.put(i, src.getShort() * k)
        } else {
            // a new level mid-song: a straight ramp over this buffer, frame by frame
            val step = (target - from) / frames.coerceAtLeast(1)
            var i = 0
            for (f in 0 until frames) {
                val k = (from + step * (f + 1)) / 32768f
                for (c in 0 until channels) {
                    floats.put(i, src.getShort() * k)
                    i++
                }
            }
        }
        appliedGain = target
        // anything left over (a partial frame) is dropped, as the sink never sends one
        inputBuffer.position(inputBuffer.limit())
        // only the gain: the engine has nothing to add, the samples go out as they are now
        if (plain) {
            writeFloats(floats, n)
            return
        }
        // null: the engine failed or went away mid-stream, the chunk is lost; the input buffer itself: no engine
        val processed = try { engine.process(frames) } catch (_: Throwable) { floats } ?: return
        if (processed === floats && outRate != inRate) return   // unprocessed input can't go out at another rate: dropped
        val outSamples = if (processed === floats) n else processed.limit()
        writeFloats(processed, outSamples)
    }

    override fun onQueueEndOfStream() {
        val tail = try { engine.flush() } catch (_: Throwable) { FloatArray(0) }
        if (tail.isNotEmpty()) writeFloats(FloatBuffer.wrap(tail), tail.size)
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

    private fun writeFloats(data: FloatBuffer, samples: Int) {
        if (outEncoding != C.ENCODING_PCM_16BIT) {
            writeWide(data, samples)
            return
        }
        val out = replaceOutputBuffer(samples * 2).order(ByteOrder.LITTLE_ENDIAN)
        for (i in 0 until samples) {
            // TPDF dither (±1 LSB triangular), then clip
            val d = (nextRand() + nextRand()) / 32768f
            val v = data.get(i) * 32768f + d
            val s = if (v > 32767f) 32767 else if (v < -32768f) -32768 else Math.round(v)
            out.putShort(s.toShort())
        }
        out.flip()
    }

    /**
     * 24-bit precision out: Cadenza's upsampled output is already dithered to 24-bit steps, so it is only scaled
     * (exactly); the effects alone at the song's rate are rounded to 24 bits.
     */
    private fun writeWide(data: FloatBuffer, samples: Int) {
        val out = replaceOutputBuffer(samples * bytesPerSample()).order(ByteOrder.LITTLE_ENDIAN)
        for (i in 0 until samples) {
            val f = data.get(i)
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
 * the app never skips silence, so they are left out and the chain's own speed stays 1. A speed other than 1 is
 * applied by the audio track instead (the sink's `setEnableAudioOutputPlaybackParameters(true)`), and only a group
 * listen's catch-up nudge sets one (0.97 / 1.03, never with bit-perfect USB or Cadenza on the phone: GroupSync);
 * no controller may change the speed (the session's onConnect leaves COMMAND_SET_SPEED_AND_PITCH out).
 */
class EffectsChain(private val effects: EffectsAudioProcessor) : AudioProcessorChain {
    private val processors: Array<AudioProcessor> = arrayOf<AudioProcessor>(effects)

    override fun getAudioProcessors(): Array<AudioProcessor> = processors

    override fun applyPlaybackParameters(playbackParameters: PlaybackParameters): PlaybackParameters = PlaybackParameters.DEFAULT

    override fun applySkipSilenceEnabled(skipSilenceEnabled: Boolean): Boolean = false

    override fun getMediaDuration(playoutDuration: Long): Long = playoutDuration

    override fun getSkippedOutputFrameCount(): Long = 0L
}
