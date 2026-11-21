package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import atrium.engine.SourceDescription
import atrium.engine.SourceTelemetry
import kotlin.math.log10
import kotlin.time.TimeSource

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

/** How much of the past the tracks can show, as a picture editor's zoom: (seconds, label). */
private val TrackZooms = listOf(30.0 to "30 s", 120.0 to "2 min", TrackMemory to "10 min")

/** The session's clock, in seconds since the app started. */
internal val trackClock = TimeSource.Monotonic.markNow()
internal fun trackSeconds(): Double = trackClock.elapsedNow().inWholeMicroseconds / 1e6

/** The share of the strip given to what is still to come: a quarter, right of now. */
private const val AheadShare = .25

/** How many moments a condition lane is drawn from. */
private const val LaneSteps = 160

/**
 * Every voice of the engine as a track, the way a multitrack editor shows
 * them: a header with the voice's number and the sound in it, the past as
 * clips, each named after its take and outlined by its level, and right of
 * the "now" line the takes the plan has already decided, drawn open. Above
 * the voices, the place's conditions as lanes, past and future alike. An
 * empty voice is a free track a sound can be added to.
 */
@Composable
internal fun TracksPanel(
    session: Session,
    selected: String?,
    onSelect: (String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    val sounds = session.description?.sources.orEmpty()
    val history = session.tracks
    val now by produceState(trackSeconds()) {
        while (true) withFrameMillis { value = trackSeconds() }
    }
    val text = rememberTextMeasurer()
    val ink = Foreground
    val faint = Hairline
    val muted = Muted
    val tint = LocalInkTint.current
    var shown by remember { mutableStateOf(TrackZooms[1].first) }
    val ahead = (shown * AheadShare).coerceAtMost(PlannedAhead.toDouble())
    val span = shown + ahead
    val lanes by produceState(emptyList<atrium.engine.Conditions>(), shown, session.engine) {
        while (true) {
            value = session.conditionsAround(-shown.toFloat(), ahead.toFloat(), LaneSteps)
            kotlinx.coroutines.delay(500)
        }
    }
    // Where a moment on the session's clock lies across the strip, 0 to 1.
    fun along(seconds: Double) = ((seconds - (now - shown)) / span).toFloat()
    val nowAt = (shown / span).toFloat()
    Column(modifier) {
        Row(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 6.dp, bottom = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            PaperText("TRACKS", 12, Muted, FontWeight.SemiBold, spacing = 2f, modifier = Modifier.weight(1f))
            TrackZooms.forEach { (seconds, label) ->
                PaperText(
                    label,
                    12,
                    if (seconds == shown) Foreground else Muted,
                    if (seconds == shown) FontWeight.Bold else FontWeight.Normal,
                    modifier = Modifier.clickable(role = Role.Tab) { shown = seconds }.padding(horizontal = 6.dp, vertical = 2.dp),
                )
            }
        }
        Row(Modifier.fillMaxWidth().padding(end = 14.dp, bottom = 2.dp)) {
            Spacer(Modifier.width(HeaderWidth))
            Ruler(shown, ahead, Modifier.weight(1f).height(14.dp))
        }
        for (condition in Condition.entries) {
            Row(Modifier.fillMaxWidth().height(LaneHeight), verticalAlignment = Alignment.CenterVertically) {
                Row(Modifier.width(HeaderWidth).padding(start = 36.dp), verticalAlignment = Alignment.CenterVertically) {
                    PaperText(condition.label, 11, Muted)
                }
                Canvas(Modifier.weight(1f).fillMaxHeight().padding(end = 14.dp)) {
                    drawLine(faint, Offset(0f, size.height), Offset(size.width, size.height), 1f)
                    if (lanes.size < 2) return@Canvas
                    val area = Path().apply {
                        moveTo(0f, size.height)
                        lanes.forEachIndexed { index, conditions ->
                            val x = size.width * index / (lanes.size - 1)
                            lineTo(x, size.height - (size.height - 2.dp.toPx()) * condition.of(conditions))
                        }
                        lineTo(size.width, size.height)
                        close()
                    }
                    drawPath(area, ink.copy(alpha = .14f))
                    drawLine(ink.copy(alpha = .5f), Offset(size.width * nowAt, 0f), Offset(size.width * nowAt, size.height), 1.dp.toPx())
                }
            }
        }
        for (slot in 0 until VoiceCount) {
            val sound = sounds.firstOrNull { it.slot.toInt() == slot }
            val audition = slot == AuditionTrack && sound == null
            val chosen = sound != null && sound.id == selected
            Row(
                Modifier.fillMaxWidth().height(TrackHeight)
                    .background(if (chosen) Highlight else Backdrop)
                    .clickable(enabled = sound != null, role = Role.Button) { onSelect(sound?.id?.takeIf { it != selected }) },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(Modifier.width(HeaderWidth).padding(start = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                    PaperText("${slot + 1}", 11, Muted, modifier = Modifier.width(22.dp))
                    val dot = if (sound == null) Hairline else tint ?: parseColor(sound.color)
                    Canvas(Modifier.width(14.dp).height(8.dp)) { drawCircle(dot, size.height / 2, Offset(size.height / 2, size.height / 2)) }
                    PaperText(
                        sound?.name ?: if (audition) "Preview" else "Empty",
                        12,
                        if (sound != null) Foreground else Muted,
                        if (chosen) FontWeight.Bold else FontWeight.Normal,
                    )
                }
                Canvas(Modifier.weight(1f).fillMaxHeight().padding(end = 14.dp)) {
                    history.version
                    val top = 2.dp.toPx()
                    val height = size.height - 2 * top
                    drawLine(faint, Offset(0f, size.height), Offset(size.width, size.height), 1f)
                    val color = when {
                        sound == null -> muted
                        chosen -> tint ?: parseColor(sound.color)
                        else -> ink
                    }
                    clipRect {
                        for (clip in history.clipsOf(slot)) {
                            val left = size.width * along(clip.start)
                            val right = (size.width * along(clip.end)).coerceAtLeast(left + 2f)
                            val alpha = if (clip.muted) .18f else if (chosen) .9f else .55f
                            drawRoundRect(color.copy(alpha = alpha * .35f), Offset(left, top), Size(right - left, height), CornerRadius(3.dp.toPx()))
                            drawOutline(clip, left, right, top, height, color.copy(alpha = alpha))
                            drawTakeName(sound?.takes?.getOrNull(clip.take)?.name, left, right, top, text, ink)
                        }
                        // What the plan has decided, still to come: open clips right of now.
                        for (planned in session.planned) {
                            if (planned.slot.toInt() != slot) continue
                            val left = size.width * along(now + planned.startsIn)
                            val right = (size.width * along(now + planned.startsIn + planned.seconds)).coerceAtLeast(left + 2f)
                            drawRoundRect(
                                color.copy(alpha = if (chosen) .8f else .45f),
                                Offset(left, top), Size(right - left, height), CornerRadius(3.dp.toPx()),
                                style = androidx.compose.ui.graphics.drawscope.Stroke(1.dp.toPx()),
                            )
                            drawTakeName(sound?.takes?.getOrNull(planned.take.toInt())?.name, left, right, top, text, muted)
                        }
                    }
                    drawLine(ink.copy(alpha = .5f), Offset(size.width * nowAt, 0f), Offset(size.width * nowAt, size.height), 1.dp.toPx())
                }
            }
        }
    }
}

/** A take's name inside its clip, when the clip is wide enough to hold it. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawTakeName(
    name: String?,
    left: Float,
    right: Float,
    top: Float,
    text: androidx.compose.ui.text.TextMeasurer,
    color: androidx.compose.ui.graphics.Color,
) {
    if (name == null || right - left <= 60.dp.toPx()) return
    val label = text.measure(name, TextStyle(fontSize = 10.sp, color = color))
    if (label.size.width < right - left - 8.dp.toPx()) {
        drawText(label, topLeft = Offset(left + 4.dp.toPx(), top + 1.dp.toPx()))
    }
}

/** The level outline inside a clip, rising from its foot like a waveform's upper half. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawOutline(
    clip: Clip,
    left: Float,
    right: Float,
    top: Float,
    height: Float,
    color: androidx.compose.ui.graphics.Color,
) {
    val levels = clip.levels
    if (levels.size < 2) return
    val foot = top + height
    val outline = Path().apply {
        moveTo(left, foot)
        levels.forEachIndexed { index, level ->
            val x = left + (right - left) * index / (levels.size - 1)
            lineTo(x, foot - height * .85f * level)
        }
        lineTo(right, foot)
        close()
    }
    drawPath(outline, color)
}

/** Time along the top of the tracks: back from now on the left, ahead of it on the right. */
@Composable
private fun Ruler(shown: Double, ahead: Double, modifier: Modifier) {
    val text = rememberTextMeasurer()
    val muted = Muted
    val span = shown + ahead
    val step = (shown / 4).toInt()
    Canvas(modifier) {
        val marks = (0..4).map { -it * step } + if (ahead >= step) listOf(step) else emptyList()
        for (secondsFromNow in marks) {
            val x = size.width * ((secondsFromNow + shown) / span).toFloat()
            val magnitude = kotlin.math.abs(secondsFromNow)
            val clock = "${magnitude / 60}:${(magnitude % 60).toString().padStart(2, '0')}"
            val label = when {
                secondsFromNow == 0 -> "now"
                secondsFromNow < 0 -> "−$clock"
                else -> "+$clock"
            }
            val measured = text.measure(label, TextStyle(fontSize = 10.sp, color = muted))
            val at = (x - measured.size.width / 2f).coerceIn(0f, size.width - measured.size.width)
            drawText(measured, topLeft = Offset(at, 0f))
        }
    }
}

private val TrackHeight = 17.dp
private val LaneHeight = 14.dp
private val HeaderWidth = 150.dp

/** The voice a preset or a take is heard from on its own; shown as Preview when no sound holds it. */
private const val AuditionTrack = VoiceCount - 1

/** A strip as high as all its tracks. */
internal val TracksPanelHeight = TrackHeight * VoiceCount + LaneHeight * Condition.entries.size + 46.dp
