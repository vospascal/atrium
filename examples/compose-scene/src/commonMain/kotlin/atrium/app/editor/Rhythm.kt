package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import atrium.engine.Gap
import atrium.engine.SourceDescription
import atrium.engine.SourceTelemetry
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlin.math.sqrt
import atrium.app.ui.Backdrop
import atrium.app.ui.Chip
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.ui.PlayIcon
import atrium.app.session.Session
import atrium.app.ui.StopIcon
import atrium.app.ui.keyboardSlider
import atrium.app.ui.listRow

/**
 * When a sound sounds: all the time, or now and then with a rest between its
 * takes; and how likely each take is. The same for anything with takes, a
 * bird, a bell, lines of a story or a tone. A generator has no takes and is
 * made fresh as it plays.
 */
@Composable
internal fun WhenPart(session: Session, sound: SourceDescription, live: SourceTelemetry?) {
    if (sound.takes.isEmpty()) {
        PaperText("All the time", 15)
        PaperText("Made fresh as it plays, so it never repeats.", 14, Muted)
        return
    }
    val gap = sound.gap
    val resting = gap != null && gap.longest > 0f
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Chip("All the time", selected = !resting) { session.gap(sound.slot, 0f, 0f) }
        Chip("Now and then", selected = resting) {
            if (!resting) session.gap(sound.slot, DefaultGap.shortest, DefaultGap.longest)
        }
    }
    Spacer(Modifier.height(10.dp))
    if (resting) {
        PaperText("Rests ${seconds(gap!!.shortest)} to ${seconds(gap.longest)} between takes", 15)
        TwoHandleRange(
            gap.shortest, gap.longest,
            toTrack = { sqrt((it / LongestRest).coerceIn(0f, 1f)) },
            fromTrack = { it * it * LongestRest },
            marks = RestMarks,
            step = .1f,
            onChange = { shortest, longest -> session.gap(sound.slot, shortest, longest) },
        )
        if (sound.takes.size > 1) {
            Spacer(Modifier.height(10.dp))
            val bout = sound.bout
            PaperText(boutLine(bout.fewest.toInt(), bout.most.toInt()), 15)
            TwoHandleRange(
                bout.fewest.toFloat(), bout.most.toFloat(),
                toTrack = { (it - 1f) / (LongestBout - 1f) },
                fromTrack = { (1f + it * (LongestBout - 1f)).roundToInt().toFloat() },
                marks = BoutMarks,
                step = 1f,
                onChange = { fewest, most -> session.bout(sound.slot, fewest.roundToInt(), most.roundToInt()) },
            )
        }
    } else if (sound.takes.size > 1) {
        PaperText("One take after another, without a rest", 14, Muted)
    } else {
        PaperText("Round and round, without a rest", 14, Muted)
    }

    if (sound.takes.size > 1 || resting) {
        Spacer(Modifier.height(14.dp))
        PaperText(if (live != null && live.take == null) "Resting" else "Sounding", 14, Muted)
        Spacer(Modifier.height(4.dp))
        sound.takes.forEachIndexed { index, take ->
            val now = live?.take?.toInt() == index
            val heard = session.isHearing(sound.slot, index)
            Row(
                Modifier.fillMaxWidth().listRow(index, selected = now).padding(top = 3.dp, bottom = 3.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    Modifier.size(28.dp).clickable(role = Role.Button) {
                        session.hearTake(sound.slot, index.takeUnless { heard })
                    },
                    contentAlignment = Alignment.Center,
                ) {
                    if (heard) StopIcon(Foreground, 12.dp) else PlayIcon(Foreground, 14.dp, outline = true)
                }
                // The name on a line of its own, and its chance under it: side by side, a narrow
                // inspector left the name a few letters a line.
                Column(Modifier.weight(1f)) {
                    PaperText(
                        take.name,
                        15,
                        if (now) Foreground else if (take.chance > 0f) Foreground.copy(alpha = .8f) else Muted,
                        if (now) FontWeight.Bold else FontWeight.Normal,
                        singleLine = true,
                    )
                    if (sound.takes.size > 1) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            PaperText("Chance", 12, Muted, singleLine = true)
                            Spacer(Modifier.width(6.dp))
                            ChanceSteps(take.chance) { session.chance(sound.slot, index, it) }
                        }
                    }
                }
            }
        }
    }
}

/** Where a sound that comes and goes starts resting: a few seconds, never the same twice. */
private val DefaultGap = Gap(2f, 8f)

/** The longest rest the range reaches (s). A scene file may give a longer one. */
private const val LongestRest = 120f

/** The longest bout the range reaches. */
private const val LongestBout = 8f

internal const val LowHandle = 0
internal const val HighHandle = 1
internal const val Undecided = -1

/**
 * Which handle a press at [fraction] of the track takes, with the handles at
 * [lowAt] and [highAt]: the nearer one. Where both sit on one spot, as a band
 * from 0 to 0 does, the side the press is on decides, and a press right on
 * them is [Undecided] until the drag shows which way it goes; otherwise the
 * low handle could never pass the high one and the range would be stuck.
 */
internal fun handleAt(fraction: Float, lowAt: Float, highAt: Float): Int {
    if (abs(lowAt - highAt) < 1e-4f) {
        return when {
            fraction > highAt + PressSlack -> HighHandle
            fraction < lowAt - PressSlack -> LowHandle
            else -> Undecided
        }
    }
    return if (abs(fraction - lowAt) <= abs(fraction - highAt)) LowHandle else HighHandle
}

/** How far from a pair of handles on one spot a press still counts as on them (share of the track). */
private const val PressSlack = .01f

/** Seconds as a range shows them: tenths below ten, whole seconds above. */
private fun seconds(value: Float): String =
    if (value < 10f) "${(value * 10).roundToInt() / 10f} s".replace(".0 s", " s") else "${value.roundToInt()} s"

/** How a sound holds on to a take, in a line. */
internal fun boutLine(fewest: Int, most: Int): String = when {
    most <= 1 -> "A new take every time"
    fewest == most -> "Each take $most times running"
    else -> "Each take $fewest to $most times running"
}

private val BoutMarks = listOf(1f to "1", 4f to "4", LongestBout to "8")

/**
 * A range with two handles: a low and a high value on one track. [toTrack]
 * and [fromTrack] map a value to its place on the track, 0 to 1, and back, so
 * a range can give more room to the values that matter most. [marks] are
 * written under the track where their values lie.
 */
@Composable
internal fun TwoHandleRange(
    low: Float,
    high: Float,
    toTrack: (Float) -> Float,
    fromTrack: (Float) -> Float,
    marks: List<Pair<Float, String>>,
    step: Float,
    onChange: (Float, Float) -> Unit,
) {
    val change by rememberUpdatedState(onChange)
    val now by rememberUpdatedState(low to high)
    val place by rememberUpdatedState(toTrack)
    val value by rememberUpdatedState(fromTrack)
    // The handle being dragged; Undecided while both sit on one spot and the drag has not yet said which way.
    var dragging by remember { mutableStateOf<Int?>(null) }
    // The handle the arrow keys move: the one last touched, the high one to begin with.
    var active by remember { mutableStateOf(HighHandle) }
    val focus = remember { FocusRequester() }
    val track = Hairline
    val ink = Foreground
    val paper = Backdrop
    fun set(handle: Int, asked: Float) {
        val (lowNow, highNow) = now
        // Never past the ends of the track, whichever way it was moved.
        val wanted = asked.coerceIn(value(0f), value(1f))
        if (handle == LowHandle) change(wanted.coerceAtMost(highNow), highNow) else change(lowNow, wanted.coerceAtLeast(lowNow))
    }
    fun moveTo(handle: Int, fraction: Float) = set(handle, value(fraction.coerceIn(0f, 1f)))
    fun nearest(fraction: Float): Int {
        val (lowNow, highNow) = now
        return handleAt(fraction, place(lowNow), place(highNow))
    }

    Box(
        Modifier.fillMaxWidth().height(34.dp)
            .keyboardSlider(focus) { steps ->
                val (lowNow, highNow) = now
                set(active, (if (active == LowHandle) lowNow else highNow) + steps * step)
            }
            .pointerInput(Unit) {
                detectTapGestures { point ->
                    focus.requestFocus()
                    val fraction = point.x / size.width
                    val handle = nearest(fraction).takeIf { it != Undecided } ?: HighHandle
                    active = handle
                    moveTo(handle, fraction)
                }
            }
            .pointerInput(Unit) {
                detectDragGestures(
                    onDragStart = { point ->
                        focus.requestFocus()
                        dragging = nearest(point.x / size.width)
                    },
                    onDragEnd = { dragging = null },
                    onDragCancel = { dragging = null },
                ) { pointer, amount ->
                    // Both on one spot: the first move says which one is pulled.
                    if (dragging == Undecided && amount.x != 0f) dragging = if (amount.x > 0f) HighHandle else LowHandle
                    dragging?.takeIf { it != Undecided }?.let {
                        active = it
                        moveTo(it, pointer.position.x / size.width)
                    }
                }
            },
    ) {
        Canvas(Modifier.fillMaxWidth().height(34.dp)) {
            val middle = size.height / 2
            val from = size.width * toTrack(low)
            val to = size.width * toTrack(high)
            drawLine(track, Offset(0f, middle), Offset(size.width, middle), strokeWidth = 2.dp.toPx())
            drawLine(ink, Offset(from, middle), Offset(to, middle), strokeWidth = 4.dp.toPx())
            for ((handle, x) in listOf(LowHandle to from, HighHandle to to)) {
                // The handle the keys move is filled.
                drawCircle(if (handle == active) ink else paper, 7.dp.toPx(), Offset(x, middle))
                drawCircle(ink, 7.dp.toPx(), Offset(x, middle), style = Stroke(2.dp.toPx()))
            }
        }
    }
    // Marks under the track, each where its value lies on it.
    BoxWithConstraints(Modifier.fillMaxWidth().height(16.dp)) {
        for ((at, label) in marks) {
            val fraction = toTrack(at)
            val labelWidth = 40.dp
            val left = (maxWidth * fraction - labelWidth * fraction).coerceAtLeast(0.dp)
            PaperText(
                label,
                12,
                Muted,
                modifier = Modifier.offset(x = left).width(labelWidth),
                align = when (fraction) {
                    0f -> TextAlign.Start
                    1f -> TextAlign.End
                    else -> TextAlign.Center
                },
            )
        }
    }
}

/** Marks on the rest range, at seconds a person thinks in. */
private val RestMarks = listOf(0f to "0", 10f to "10 s", 30f to "30 s", 60f to "1 min", LongestRest to "2 min")

/** How often a take is chosen, as steps from never (0) to four times as often as one. */
private const val ChanceStepCount = 4

/**
 * A take's chance as a row of steps: the filled ones are its chance. The ring
 * before them is never; pressing it stops the take being chosen at all.
 */
@Composable
private fun ChanceSteps(chance: Float, onChange: (Float) -> Unit) {
    val ink = Foreground
    val faint = Hairline
    val steps = chance.roundToInt().coerceIn(0, ChanceStepCount)
    Row(verticalAlignment = Alignment.CenterVertically) {
        Canvas(Modifier.size(24.dp).clickable(role = Role.Button) { onChange(0f) }.padding(6.dp)) {
            drawCircle(if (steps == 0) ink else faint, size.minDimension / 2, style = Stroke(1.5.dp.toPx()))
            if (steps == 0) drawLine(ink, Offset(0f, size.height), Offset(size.width, 0f), 1.5.dp.toPx())
        }
        for (step in 1..ChanceStepCount) {
            Canvas(Modifier.size(width = 16.dp, height = 24.dp).clickable(role = Role.Button) { onChange(step.toFloat()) }.padding(horizontal = 2.dp, vertical = 7.dp)) {
                val filled = step <= steps
                drawRoundRect(
                    if (filled) ink else faint,
                    size = Size(size.width, size.height),
                    cornerRadius = CornerRadius(2.dp.toPx()),
                    style = if (filled) androidx.compose.ui.graphics.drawscope.Fill else Stroke(1.dp.toPx()),
                )
            }
        }
        Spacer(Modifier.width(6.dp))
        PaperText(if (steps == 0) "never" else "${steps}×", 13, Muted, modifier = Modifier.width(38.dp))
    }
}
