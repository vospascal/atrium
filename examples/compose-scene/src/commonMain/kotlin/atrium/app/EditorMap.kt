package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.withFrameMillis
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.sp
import atrium.engine.EmitterKind
import atrium.engine.Position
import kotlin.math.PI
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.roundToInt
import kotlin.math.sin

/** A sound as the map draws it: where it is now, relative to the listener. */
private data class Marker(
    val slot: UShort,
    val id: String,
    val name: String,
    val offset: Point,
    /** Metres above the ground. */
    val height: Float,
    val color: Color,
    /** How far it reaches before it has faded to a whisper (m). */
    val reach: Float,
    val ambient: Boolean,
    val beamHalfAngle: Float?,
    val facing: Float,
    val strength: Float,
    val silent: Boolean,
) {
    /** A sound with a beam can be turned; an ambient or omni one sounds the same whichever way it faces. */
    val turnable: Boolean get() = !ambient && beamHalfAngle != null
}

/** A sound whose facing handle is being dragged: the map owns its heading until the drag ends. */
private data class Turned(val slot: UShort, val heading: Float)

/** A sound being dragged: the map owns its place until the engine reports it there. */
private data class Dragged(val slot: UShort, val offset: Point)

/**
 * Every sound of the playing scene from above, in the player's paper style.
 * Click a sound to select it, drag it to move it, click the ground to select
 * nothing. A sound is black and white until it is selected. Its circle, or
 * its wedge when it is focused, is how far its level carries, and a ring
 * travels outward while it is heard. An ambient sound is bands arriving from
 * the side its anchor lies on, and can be grabbed near that anchor. The
 * selected focused sound has a handle at the end of its facing line: drag it
 * to turn the sound.
 */
@Composable
internal fun EditorMap(
    session: Session,
    strengths: List<Float>,
    selected: String?,
    onSelect: (String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = LocalPlayerColors.current
    val tint = LocalInkTint.current
    val text = rememberTextMeasurer()
    val description = session.description
    val telemetry = session.telemetry
    val listener = description?.listener ?: Position(0f, 0f, 0f)
    var dragged by remember(session.engine) { mutableStateOf<Dragged?>(null) }
    var turned by remember(session.engine) { mutableStateOf<Turned?>(null) }
    val echo = remember(session.engine) { DragEcho() }

    // Hand a dragged sound back to telemetry once the engine reports it where it was dropped.
    LaunchedEffect(telemetry) {
        val held = dragged ?: return@LaunchedEffect
        val reported = telemetry?.sources?.firstOrNull { it.slot == held.slot }?.position ?: return@LaunchedEffect
        if (echo.accepts(Point(reported.x - listener.x, reported.y - listener.y))) dragged = null
    }

    val markers = description?.sources.orEmpty().mapIndexed { index, sound ->
        val live = telemetry?.sources?.firstOrNull { it.slot == sound.slot }
        val at = live?.position ?: sound.position
        val held = dragged?.takeIf { it.slot == sound.slot }
        val facing = live?.facing ?: sound.facing
        Marker(
            sound.slot,
            sound.id,
            sound.name,
            held?.offset ?: Point(at.x - listener.x, at.y - listener.y),
            height = at.z,
            // Black and white until it is selected; then its own ink.
            if (sound.id == selected) tint ?: parseColor(sound.color) else colors.foreground,
            reach = reachOf(sound.spl, sound.referenceDistance),
            ambient = sound.emitterKind == EmitterKind.FIELD,
            beamHalfAngle = sound.beamHalfAngle,
            facing = turned?.takeIf { it.slot == sound.slot }?.heading ?: atan2(facing.y, facing.x),
            strength = strengths.getOrElse(index) { 0f },
            silent = session.mix.silences(sound.id),
        )
    }
    // Sized by where sounds were placed, so the view holds still while one is dragged or moves by itself.
    val reach = reachFor(description?.sources.orEmpty().map { Point(it.position.x - listener.x, it.position.y - listener.y) })
    val current = rememberUpdatedState(markers)
    val select = rememberUpdatedState(onSelect)
    val chosen = rememberUpdatedState(selected)

    /** The sound a pointer at [at] picks in [view], if any. */
    fun pick(view: TopDown, at: Offset, density: Float): Marker? {
        val (washes, points) = current.value.withIndex().partition { it.value.ambient }
        val index = pickAt(
            points.map { it.index to view.toScreen(it.value.offset) },
            washes.map { it.index to view.toScreen(it.value.offset) },
            AmbientRadius * view.pixelsPerMetre,
            at,
            PickRadius * density,
        )
        return index?.let { current.value[it] }
    }

    // Seconds since the map appeared: what the travelling rings and drifting bands move by.
    val beat by produceState(0f) {
        val start = withFrameMillis { it }
        while (true) withFrameMillis { value = (it - start) / 1000f }
    }

    Canvas(
        modifier
            .clipToBounds()
            .pointerInput(session.engine, reach) {
                detectTapGestures { at ->
                    val view = TopDown(androidx.compose.ui.geometry.Size(size.width.toFloat(), size.height.toFloat()), reach)
                    select.value(pick(view, at, density)?.id)
                }
            }
            .pointerInput(session.engine, reach) {
                var view: TopDown? = null
                detectDragGestures(
                    onDragStart = { at ->
                        val top = TopDown(androidx.compose.ui.geometry.Size(size.width.toFloat(), size.height.toFloat()), reach)
                        view = top
                        // The facing handle of the selected sound is tried first: it sits away from its dot.
                        val focused = current.value.firstOrNull { it.id == chosen.value && it.turnable }
                        val handle = focused?.let { facingHandle(top.toScreen(it.offset), it.facing, FacingReach * density) }
                        if (focused != null && handle != null && (handle - at).getDistance() <= HandleRadius * density) {
                            turned = Turned(focused.slot, focused.facing)
                            return@detectDragGestures
                        }
                        val marker = pick(top, at, density)
                        if (marker != null) {
                            select.value(marker.id)
                            echo.move()
                            dragged = Dragged(marker.slot, marker.offset)
                        }
                    },
                    onDrag = { change, _ ->
                        val turning = turned
                        val top = view
                        if (turning != null && top != null) {
                            val sound = current.value.firstOrNull { it.slot == turning.slot }
                            val toward = sound?.let { groundDirection(top.toScreen(it.offset), change.position) }
                            if (toward != null && toward.hasGroundBearing()) {
                                turned = turning.copy(heading = atan2(toward.y, toward.x))
                                session.face(turning.slot, Position(toward.x, toward.y, 0f))
                            }
                            return@detectDragGestures
                        }
                        val held = dragged
                        if (held != null && top != null) {
                            val at = top.toWorld(change.position)
                            val height = current.value.firstOrNull { it.slot == held.slot }?.height ?: 0f
                            dragged = held.copy(offset = at)
                            session.move(held.slot, Position(listener.x + at.x, listener.y + at.y, height))
                        }
                    },
                    onDragEnd = {
                        dragged?.let { echo.release(it.offset) }
                        turned = null
                        session.placed()
                    },
                    onDragCancel = {
                        dragged?.let { echo.release(it.offset) }
                        turned = null
                        session.placed()
                    },
                )
            },
    ) {
        val view = TopDown(size, reach)
        val centre = view.toScreen(Point(0f, 0f))

        ringsWithin(reach).forEach { metres ->
            drawCircle(colors.hairline, metres * view.pixelsPerMetre, centre, style = Stroke(width = density * .8f))
            val label = text.measure("${metres.roundToInt()} m", TextStyle(color = colors.muted, fontSize = 11.sp))
            drawText(label, topLeft = centre + Offset(density * 4f, -metres * view.pixelsPerMetre - label.size.height))
        }
        val north = text.measure("N", TextStyle(color = colors.muted, fontSize = 11.sp))
        drawText(north, topLeft = Offset(centre.x - north.size.width / 2f, density * 6f))

        // Ambient washes first, so points and wedges stay readable on top of them.
        markers.filter { it.ambient }.forEach { drawAmbient(it, centre, view, colors, text, it.id == selected, beat) }
        // Reaches under every point, so a small sound inside a large one can still be seen and picked.
        markers.filterNot { it.ambient }.sortedByDescending { it.reach }.forEach { drawReach(view.toScreen(it.offset), it, view, beat, it.id == selected) }

        drawListener(centre, telemetry?.listenerFacing, colors)

        markers.filterNot { it.ambient }.forEach { marker ->
            val at = view.toScreen(marker.offset)
            if (marker.id == selected) drawBearingLine(centre, at, marker.offset, colors, text)
            if (marker.id == selected && marker.turnable) drawFacingHandle(at, marker, colors)
            drawPoint(at, marker, colors, text, marker.id == selected)
        }
    }
}

private fun DrawScope.drawListener(centre: Offset, facing: Position?, colors: PlayerColors) {
    if (facing != null) {
        val heading = atan2(facing.y, facing.x)
        val reach = density * ListenerWedge
        val wedge = Path().apply {
            moveTo(centre.x, centre.y)
            for (step in 0..WedgeSteps) {
                val angle = heading - ListenerSpread + 2 * ListenerSpread * step / WedgeSteps
                lineTo(centre.x + cos(angle) * reach, centre.y - sin(angle) * reach)
            }
            close()
        }
        drawPath(wedge, colors.foreground.copy(alpha = .14f))
        drawLine(
            colors.foreground.copy(alpha = .6f),
            centre,
            Offset(centre.x + cos(heading) * reach, centre.y - sin(heading) * reach),
            strokeWidth = density * 1.4f,
        )
    }
    drawCircle(colors.backdrop, density * 6f, centre)
    drawCircle(colors.foreground, density * 6f, centre, style = Stroke(width = density * 1.6f))
    drawCircle(colors.foreground, density * 2f, centre)
}

/**
 * An ambient sound is heard everywhere, so it has no circle of its own: broad
 * bands arriving from the side its anchor lies on, or rings all round when
 * the anchor is at the listener. The small mark is the anchor a drag moves.
 */
private fun DrawScope.drawAmbient(marker: Marker, centre: Offset, view: TopDown, colors: PlayerColors, text: TextMeasurer, selected: Boolean, beat: Float) {
    val at = view.toScreen(marker.offset)
    val alpha = if (marker.silent) .10f else .22f + .30f * marker.strength
    val stroke = Stroke(width = density * 7f, cap = androidx.compose.ui.graphics.StrokeCap.Round)
    val from = atan2(marker.offset.y, marker.offset.x)
    val surround = !marker.offset.copy(z = 0f).hasGroundBearing() || hypot(marker.offset.x, marker.offset.y) < SurroundWithin
    repeat(AmbientBands) { band ->
        // The bands drift toward the listener while the wind is heard.
        val drift = if (marker.silent) 0f else (beat * BandDrift) % 1f
        val radius = (AmbientNearest + (band + 1f - drift) * AmbientBandGap) * view.pixelsPerMetre
        val fade = alpha * (1f - (band + 1f - drift) / (AmbientBands + 1f))
        if (surround) {
            drawCircle(marker.color.copy(alpha = fade), radius, centre, style = stroke)
        } else {
            // Compose measures angles clockwise from east; the map's north is up.
            drawArc(
                marker.color.copy(alpha = fade),
                startAngle = -(from + AmbientSpread) * 180f / PI.toFloat(),
                sweepAngle = 2 * AmbientSpread * 180f / PI.toFloat(),
                useCenter = false,
                topLeft = Offset(centre.x - radius, centre.y - radius),
                size = androidx.compose.ui.geometry.Size(radius * 2, radius * 2),
                style = stroke,
            )
        }
    }
    drawCircle(marker.color.copy(alpha = if (marker.silent) .4f else .9f), density * 3f, at)
    if (selected) drawCircle(colors.foreground, density * 8f, at, style = Stroke(width = density * 1.6f))
    drawName(marker, at, colors, text, selected)
}

/** The handle that turns the selected sound: a line the way it faces, with a ring to grab at its end. */
private fun DrawScope.drawFacingHandle(at: Offset, marker: Marker, colors: PlayerColors) {
    val handle = facingHandle(at, marker.facing, FacingReach * density)
    drawLine(colors.foreground, at, handle, strokeWidth = density * 1.4f)
    drawCircle(colors.backdrop, density * 6f, handle)
    drawCircle(colors.foreground, density * 6f, handle, style = Stroke(width = density * 1.6f))
}

/**
 * How far a sound reaches, drawn as its shape: a full circle for one that is
 * equal all round, a wedge as wide as its beam for a focused one. Both have a
 * border, and a ring travels outward from the sound while it is heard.
 */
private fun DrawScope.drawReach(at: Offset, marker: Marker, view: TopDown, beat: Float, selected: Boolean) {
    val radius = marker.reach * view.pixelsPerMetre
    val heard = !marker.silent && marker.strength > 0f
    // Only the selected sound is filled, so overlapping reaches stay readable as outlines.
    val fill = marker.color.copy(alpha = if (selected) .12f else 0f)
    val edge = marker.color.copy(alpha = if (selected) .9f else if (marker.silent) .18f else .38f)
    val border = Stroke(width = density * 1.2f)
    val half = marker.beamHalfAngle
    if (half == null) {
        drawCircle(fill, radius, at)
        drawCircle(edge, radius, at, style = border)
        if (heard) {
            val travelled = (beat * PulseRate + marker.slot.toInt() * .37f) % 1f
            drawCircle(marker.color.copy(alpha = (1f - travelled) * .5f * marker.strength), radius * travelled, at, style = border)
        }
        return
    }
    fun wedge(length: Float) = Path().apply {
        moveTo(at.x, at.y)
        for (step in 0..WedgeSteps) {
            val angle = marker.facing - half + 2 * half * step / WedgeSteps
            lineTo(at.x + cos(angle) * length, at.y - sin(angle) * length)
        }
        close()
    }
    drawPath(wedge(radius), fill)
    drawPath(wedge(radius), edge, style = border)
    if (heard) {
        val travelled = (beat * PulseRate + marker.slot.toInt() * .37f) % 1f
        drawPath(wedge(radius * travelled), marker.color.copy(alpha = (1f - travelled) * .5f * marker.strength), style = border)
    }
}

private fun DrawScope.drawPoint(at: Offset, marker: Marker, colors: PlayerColors, text: TextMeasurer, selected: Boolean) {
    if (selected) drawCircle(colors.foreground, density * 11f, at, style = Stroke(width = density * 1.6f))
    if (marker.silent) {
        drawCircle(colors.backdrop, density * 6f, at)
        drawCircle(marker.color, density * 6f, at, style = Stroke(width = density * 1.5f))
    } else {
        drawCircle(marker.color, density * 6f, at)
    }
    drawName(marker, at, colors, text, selected)
}

private fun DrawScope.drawName(marker: Marker, at: Offset, colors: PlayerColors, text: TextMeasurer, selected: Boolean) {
    val name = text.measure(
        marker.name,
        TextStyle(color = if (marker.silent) colors.muted else colors.foreground, fontSize = 13.sp),
    )
    val left = at + Offset(density * (if (selected) 16f else 11f), -name.size.height / 2f)
    drawText(name, topLeft = left)
    if (marker.height > HeightWorthSaying) {
        val height = text.measure("${oneDecimal(marker.height)} m up", TextStyle(color = colors.muted, fontSize = 11.sp))
        drawText(height, topLeft = left + Offset(0f, name.size.height.toFloat() - density * 1f))
    }
}

/** The selected sound's distance and bearing, on a dashed line from the listener. */
private fun DrawScope.drawBearingLine(centre: Offset, at: Offset, offset: Point, colors: PlayerColors, text: TextMeasurer) {
    drawLine(
        colors.foreground.copy(alpha = .55f),
        centre,
        at,
        strokeWidth = density * 1f,
        pathEffect = PathEffect.dashPathEffect(floatArrayOf(density * 4f, density * 4f)),
    )
    val label = text.measure(awayLabel(offset), TextStyle(color = colors.muted, fontSize = 11.sp))
    val middle = Offset((centre.x + at.x) / 2f, (centre.y + at.y) / 2f)
    drawText(label, topLeft = middle + Offset(density * 6f, -label.size.height - density * 2f))
}

/** "14.3 m · 45° NE": how far a sound is from the listener over the ground, and in which direction. */
internal fun awayLabel(offset: Point): String =
    "${oneDecimal(hypot(offset.x, offset.y))} m · ${bearingLabel(Position(offset.x, offset.y, 0f))}"

/** A number with one decimal, without a platform formatter. */
internal fun oneDecimal(value: Float): String {
    val tenths = (value * 10).roundToInt()
    val sign = if (tenths < 0) "-" else ""
    return "$sign${kotlin.math.abs(tenths) / 10}.${kotlin.math.abs(tenths) % 10}"
}

/** How close to a sound a click or the start of a drag has to be (dp). */
private const val PickRadius = 20f

/** How far from its sound the facing handle sits, and how close a drag has to start to grab it (dp). */
private const val FacingReach = 46f
private const val HandleRadius = 16f
private const val ListenerWedge = 40f
private const val ListenerSpread = (35.0 * PI / 180.0).toFloat()
private const val WedgeSteps = 12

/** How near its anchor an ambient sound can be grabbed (m). */
private const val AmbientRadius = 6f

/** An ambient sound's bands: how many, how far from the listener the nearest is and how far apart (m), and how wide (rad). */
private const val AmbientBands = 4
private const val AmbientNearest = 3f
private const val AmbientBandGap = 3f
private const val AmbientSpread = (50.0 * PI / 180.0).toFloat()

/** An anchor this close to the listener means the sound comes from all round (m). */
private const val SurroundWithin = 1f

/** How fast a band drifts inward and a ring travels outward (per second). */
private const val BandDrift = .12f
private const val PulseRate = .35f

/** A sound this far above the ground gets its height written under its name (m). */
private const val HeightWorthSaying = .3f
