package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
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
    val slot: Int,
    val id: String,
    val name: String,
    val offset: Point,
    /** Metres above the ground. */
    val height: Float,
    val color: Color,
    val ambient: Boolean,
    val beamHalfAngle: Float?,
    val facing: Float,
    val strength: Float,
    val silent: Boolean,
)

/** A sound being dragged: the map owns its place until the engine reports it there. */
private data class Dragged(val slot: Int, val offset: Point)

/**
 * Every sound of the playing scene from above, in the player's paper style.
 * Click a sound to select it, drag it to move it, click the ground to select
 * nothing. An ambient sound is a wash, a focused one a wedge, the rest a dot
 * with a halo as strong as it is heard.
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
    val echo = remember(session.engine) { DragEcho() }

    // Hand a dragged sound back to telemetry once the engine reports it where it was dropped.
    LaunchedEffect(telemetry) {
        val held = dragged ?: return@LaunchedEffect
        val reported = telemetry?.sources?.firstOrNull { it.slot.toInt() == held.slot }?.position ?: return@LaunchedEffect
        if (echo.accepts(Point(reported.x - listener.x, reported.y - listener.y))) dragged = null
    }

    val markers = description?.sources.orEmpty().mapIndexed { slot, sound ->
        val live = telemetry?.sources?.firstOrNull { it.slot.toInt() == slot }
        val at = live?.position ?: sound.position
        val held = dragged?.takeIf { it.slot == slot }
        val facing = live?.facing ?: sound.facing
        Marker(
            slot,
            sound.id,
            sound.name,
            held?.offset ?: Point(at.x - listener.x, at.y - listener.y),
            height = at.z,
            tint ?: parseColor(sound.color),
            ambient = sound.emitterKind == EmitterKind.FIELD,
            beamHalfAngle = sound.beamHalfAngle,
            facing = atan2(facing.y, facing.x),
            strength = strengths.getOrElse(slot) { 0f },
            silent = session.mix.silences(sound.id),
        )
    }
    // Sized by where sounds were placed, so the view holds still while one is dragged or moves by itself.
    val reach = reachFor(description?.sources.orEmpty().map { Point(it.position.x - listener.x, it.position.y - listener.y) })
    val current = rememberUpdatedState(markers)
    val select = rememberUpdatedState(onSelect)

    Canvas(
        modifier
            .pointerInput(session.engine, reach) {
                detectTapGestures { at ->
                    val view = TopDown(androidx.compose.ui.geometry.Size(size.width.toFloat(), size.height.toFloat()), reach)
                    val hit = nearestWithin(current.value.map { view.toScreen(it.offset) }, at, PickRadius * density)
                    select.value(hit?.let { current.value[it].id })
                }
            }
            .pointerInput(session.engine, reach) {
                var view: TopDown? = null
                detectDragGestures(
                    onDragStart = { at ->
                        val top = TopDown(androidx.compose.ui.geometry.Size(size.width.toFloat(), size.height.toFloat()), reach)
                        view = top
                        val hit = nearestWithin(current.value.map { top.toScreen(it.offset) }, at, PickRadius * density)
                        val marker = hit?.let { current.value[it] }
                        if (marker != null) {
                            select.value(marker.id)
                            echo.move()
                            dragged = Dragged(marker.slot, marker.offset)
                        }
                    },
                    onDrag = { change, _ ->
                        val held = dragged
                        val top = view
                        if (held != null && top != null) {
                            val at = top.toWorld(change.position)
                            val height = current.value.firstOrNull { it.slot == held.slot }?.height ?: 0f
                            dragged = held.copy(offset = at)
                            session.move(held.slot, Position(listener.x + at.x, listener.y + at.y, height))
                        }
                    },
                    onDragEnd = {
                        dragged?.let { echo.release(it.offset) }
                        session.placed()
                    },
                    onDragCancel = {
                        dragged?.let { echo.release(it.offset) }
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
        markers.filter { it.ambient }.forEach { drawAmbient(it, view, colors, text, it.id == selected) }

        drawListener(centre, telemetry?.listenerFacing, colors)

        markers.filterNot { it.ambient }.forEach { marker ->
            val at = view.toScreen(marker.offset)
            if (marker.id == selected) drawBearingLine(centre, at, marker.offset, colors, text)
            marker.beamHalfAngle?.let { drawBeam(at, marker, it, view) }
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

/** An ambient sound has no place it comes from: a soft wash around its anchor, with a dashed edge. */
private fun DrawScope.drawAmbient(marker: Marker, view: TopDown, colors: PlayerColors, text: TextMeasurer, selected: Boolean) {
    val at = view.toScreen(marker.offset)
    val radius = AmbientRadius * view.pixelsPerMetre
    val alpha = if (marker.silent) .03f else .05f + .10f * marker.strength
    drawCircle(marker.color.copy(alpha = alpha), radius, at)
    drawCircle(
        marker.color.copy(alpha = if (selected) .9f else .45f),
        radius,
        at,
        style = Stroke(width = density * if (selected) 1.8f else 1f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(density * 6f, density * 5f))),
    )
    drawName(marker, at, colors, text, selected)
}

/** A focused sound: a wedge as wide as its beam, pointing where it faces. */
private fun DrawScope.drawBeam(at: Offset, marker: Marker, halfAngle: Float, view: TopDown) {
    val length = BeamLength * view.pixelsPerMetre
    val wedge = Path().apply {
        moveTo(at.x, at.y)
        for (step in 0..WedgeSteps) {
            val angle = marker.facing - halfAngle + 2 * halfAngle * step / WedgeSteps
            lineTo(at.x + cos(angle) * length, at.y - sin(angle) * length)
        }
        close()
    }
    drawPath(wedge, marker.color.copy(alpha = if (marker.silent) .06f else .2f))
}

private fun DrawScope.drawPoint(at: Offset, marker: Marker, colors: PlayerColors, text: TextMeasurer, selected: Boolean) {
    if (!marker.silent && marker.strength > 0f) {
        val halo = density * (14f + 26f * marker.strength)
        drawCircle(
            Brush.radialGradient(
                0f to marker.color.copy(alpha = .40f * marker.strength + .08f),
                1f to marker.color.copy(alpha = 0f),
                center = at,
                radius = halo,
            ),
            halo,
            at,
        )
    }
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
private const val ListenerWedge = 40f
private const val ListenerSpread = (35.0 * PI / 180.0).toFloat()
private const val WedgeSteps = 12

/** How large an ambient sound's wash is drawn, and how long a beam (m). */
private const val AmbientRadius = 6f
private const val BeamLength = 6f

/** A sound this far above the ground gets its height written under its name (m). */
private const val HeightWorthSaying = .3f
