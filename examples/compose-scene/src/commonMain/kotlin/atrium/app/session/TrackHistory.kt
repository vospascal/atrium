package atrium.app.session

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import atrium.engine.SourceTelemetry
import kotlin.math.log10

/**
 * One stretch of time a voice was sounding: from [start] to [end] seconds on
 * the session's clock, uttering [take] (counted from 0), with its level over
 * that time as [levels], 0 to 1, one every [LevelStep] seconds.
 */
internal data class Clip(val start: Double, var end: Double, val take: Int, val muted: Boolean, val levels: MutableList<Float>)

/** How often a clip keeps its level (s): enough for an outline, few enough to draw cheaply. */
internal const val LevelStep = .1

/**
 * What every voice did over the last [window] seconds, as clips: built from
 * telemetry as it arrives, so it is what was heard, not what was planned. A
 * voice that rests between takes leaves gaps; one that loops is one long clip.
 */
@Stable
internal class TrackHistory(private val window: Double = TrackMemory) {
    private val clips = List(VoiceCount) { ArrayDeque<Clip>() }
    private val lastLevelAt = DoubleArray(VoiceCount) { Double.NEGATIVE_INFINITY }

    /** Bumped on every change, so a drawing that reads this history knows to draw again. */
    var version by mutableStateOf(0)
        private set

    /** Take in what the voices are doing at [now] seconds. A voice missing from [sources] is silent. */
    fun record(now: Double, sources: List<SourceTelemetry>) {
        val bySlot = sources.associateBy { it.slot.toInt() }
        for (slot in 0 until VoiceCount) {
            val track = clips[slot]
            val source = bySlot[slot]
            val take = source?.take?.toInt()
            val open = track.lastOrNull()?.takeIf { it.end >= now - Continuous }
            when {
                take == null -> Unit
                open != null && open.take == take && open.muted == source.muted -> {
                    open.end = now
                    if (now - lastLevelAt[slot] >= LevelStep) {
                        open.levels += levelOf(source)
                        lastLevelAt[slot] = now
                    }
                }
                else -> {
                    track += Clip(now, now, take, source.muted, mutableListOf(levelOf(source)))
                    lastLevelAt[slot] = now
                }
            }
            while (track.isNotEmpty() && track.first().end < now - window) track.removeFirst()
        }
        version++
    }

    /** The clips of the voice in [slot], oldest first. */
    fun clipsOf(slot: Int): List<Clip> = clips[slot]

    /** Forget everything: a new scene starts with empty tracks. */
    fun clear() {
        clips.forEach { it.clear() }
        lastLevelAt.fill(Double.NEGATIVE_INFINITY)
        version++
    }

    private companion object {
        /** Frames closer than this belong to one clip; a longer silence starts a new one (s). */
        const val Continuous = .25
    }
}

/** A voice's own level as a clip outline shows it, 0 to 1: its measured loudness over 36 dB. */
private fun levelOf(source: SourceTelemetry): Float {
    val rms = source.signal?.relativeRms ?: return .7f
    if (rms <= 0f) return 0f
    return ((20f * log10(rms) + 36f) / 36f).coerceIn(.08f, 1f)
}

/** How much the tracks keep (s): the widest they can show. */
internal const val TrackMemory = 600.0

