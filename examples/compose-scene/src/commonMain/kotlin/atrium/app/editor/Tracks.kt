package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.ui.draw.drawBehind
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.ui.input.pointer.pointerHoverIcon
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
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
import atrium.app.session.Clip
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ink.LocalInkTint
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.session.PlannedAhead
import atrium.app.ui.ScrollArea
import atrium.app.session.Session
import atrium.app.session.TrackMemory
import atrium.app.session.VoiceCount
import atrium.app.ui.labelsThatFit
import atrium.app.ui.listRow
import atrium.app.ink.parseColor
import atrium.app.ui.resizePointer

/** How much of the past the tracks can show, as a picture editor's zoom: (seconds, label). */
private val TrackZooms = listOf(30.0 to "30 s", 120.0 to "2 min", TrackMemory to "10 min")

/** The share of the strip given to what is still to come: a quarter, right of now. */
private const val AheadShare = .25

/** How many moments a condition lane is drawn from. */
private const val LaneSteps = 160

/** What the bottom panel shows under its timeline. */
internal enum class BottomTab { Tracks, Analysis, Lab }

/** Seconds into the playing scene, read every frame, as the ruler's "now". */
@Composable
private fun playhead(session: Session): Double {
    val now by produceState(session.sceneTime()) {
        while (true) withFrameMillis { value = session.sceneTime() }
    }
    return now
}

/**
 * The panel under the map, as a video editor's: on top the timeline, the meters and the zoom,
 * and the ruler; under them three tabs. Tracks shows the voices and the place's conditions and
 * Analysis what was heard, measured as it played, both on the ruler's time axis; the Lab tunes a
 * generator against a recording, on its own playhead. The tab chosen is kept in [workspace]. [now] is the scene's time at the ruler's "now" line.
 */
@Composable
internal fun BottomPanel(
    session: Session,
    workspace: Workspace,
    selection: Selection,
    onSelection: (Selection) -> Unit,
    modifier: Modifier = Modifier,
    now: Double = playhead(session),
) {
    var shown by remember { mutableStateOf(TrackZooms[1].first) }
    val ahead = (shown * AheadShare).coerceAtMost(PlannedAhead.toDouble())
    val span = shown + ahead
    Column(modifier) {
        Overview(session, now, Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 6.dp, bottom = 4.dp)) {
            Spacer(Modifier.width(14.dp))
            ChannelMeters(session)
            Spacer(Modifier.width(4.dp))
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
            // Dragging the ruler scrubs: the scene moves as far as the drag goes along it.
            Ruler(
                shown, ahead,
                Modifier.weight(1f).height(16.dp)
                    .pointerHoverIcon(resizePointer(acrossWidth = true))
                    .pointerInput(span) {
                        detectHorizontalDragGestures { change, amount ->
                            change.consume()
                            session.seek(session.sceneTime() + amount / size.width * span)
                        }
                    },
            )
        }
        BottomTabs(workspace.bottomTab) { workspace.bottomTab = it }
        when (workspace.bottomTab) {
            BottomTab.Tracks -> TrackLanes(session, now, shown, ahead, selection, onSelection, Modifier.weight(1f))
            BottomTab.Analysis -> AnalysisLanes(session.analysis, now, shown, ahead, Modifier.weight(1f))
            BottomTab.Lab -> LabPanel(session.lab, Modifier.weight(1f))
        }
    }
}

/** The bottom panel's tabs, in the look of the side panel's: the chosen one bold, a hairline under them. */
@Composable
private fun BottomTabs(selected: BottomTab, onSelect: (BottomTab) -> Unit) {
    val line = Hairline
    Row(
        Modifier.fillMaxWidth().padding(start = 8.dp, end = 14.dp, top = 2.dp)
            .drawBehind { drawLine(line, Offset(0f, size.height - .5f), Offset(size.width, size.height - .5f), 1f) },
    ) {
        BottomTab.entries.forEach { tab ->
            PaperText(
                tab.name,
                13,
                if (tab == selected) Foreground else Muted,
                if (tab == selected) FontWeight.Bold else FontWeight.Normal,
                singleLine = true,
                modifier = Modifier.clickable(role = Role.Tab) { onSelect(tab) }.padding(horizontal = 6.dp, vertical = 4.dp),
            )
        }
    }
}

/**
 * Every voice of the engine as a track, the way a multitrack editor shows
 * them: a header with the voice's number and the sound in it, the past as
 * clips, each named after its take and outlined by its level, and right of
 * the "now" line the takes the plan has already decided, drawn open. Above
 * the voices, the place's conditions as lanes, past and future alike. An
 * empty voice is a free track a sound can be added to.
 */
@Composable
private fun TrackLanes(
    session: Session,
    now: Double,
    shown: Double,
    ahead: Double,
    selection: Selection,
    onSelection: (Selection) -> Unit,
    modifier: Modifier = Modifier,
) {
    val sounds = session.description?.sources.orEmpty()
    val keys = remember { HeldKeys() }
    val history = session.tracks
    val text = rememberTextMeasurer()
    val ink = Foreground
    val faint = Hairline
    val muted = Muted
    val tint = LocalInkTint.current
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
    // The lanes and the voices scroll together under the fixed ruler when the panel is short.
    ScrollArea(modifier) {
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
            val chosen = sound != null && sound.id in selection
            Row(
                keys.watch.fillMaxWidth().height(TrackHeight)
                    .listRow(slot, chosen)
                    .clickable(enabled = sound != null, role = Role.Button) { sound?.let { onSelection(selection.clickedInList(it.id, keys.adding)) } },
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
        // Now first, then outward from it: in a narrow panel the ones farthest back give way.
        val marks = listOf(0) + (if (ahead >= step) listOf(step) else emptyList()) + (1..4).map { -it * step }
        val labels = marks.map { secondsFromNow ->
            val x = size.width * ((secondsFromNow + shown) / span).toFloat()
            val magnitude = kotlin.math.abs(secondsFromNow)
            val clock = "${magnitude / 60}:${(magnitude % 60).toString().padStart(2, '0')}"
            val label = when {
                secondsFromNow == 0 -> "now"
                secondsFromNow < 0 -> "−$clock"
                else -> "+$clock"
            }
            val measured = text.measure(label, TextStyle(fontSize = 10.sp, color = muted))
            measured to (x - measured.size.width / 2f).coerceIn(0f, size.width - measured.size.width)
        }
        val kept = labelsThatFit(labels.map { (measured, at) -> at..(at + measured.size.width) }, 6.dp.toPx())
        labels.forEachIndexed { index, (measured, at) ->
            if (index in kept) drawText(measured, topLeft = Offset(at, 0f))
        }
    }
}

private val TrackHeight = 17.dp
private val LaneHeight = 14.dp
/** The width of the lanes' names, left of the strip that lines up with the ruler. */
internal val HeaderWidth = 150.dp

/** The voice a preset or a take is heard from on its own; shown as Preview when no sound holds it. */
private const val AuditionTrack = VoiceCount - 1
