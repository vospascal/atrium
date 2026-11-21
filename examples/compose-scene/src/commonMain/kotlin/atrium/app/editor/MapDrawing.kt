package atrium.app.editor

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.unit.sp
import atrium.engine.Position
import kotlin.math.PI
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.roundToInt
import kotlin.math.sin
import atrium.app.ui.PlayerColors
import atrium.app.Point
import atrium.app.bearingLabel
import atrium.app.compassLabel

/**
 * Which way the wind blows, in the map's top-left corner: an arrow pointing where it goes, on a
 * map with north up, and the bearing it comes from.
 */
internal fun DrawScope.drawWind(fromDegrees: Float, colors: PlayerColors, text: TextMeasurer) {
    val middle = Offset(density * 30f, density * 30f)
    val (east, north) = windToward(fromDegrees)
    val half = density * WindArrow
    // North is up on the map, so north is toward the top of the screen.
    val along = Offset(east * half, -north * half)
    val tip = middle + along
    drawLine(colors.muted, middle - along, tip, strokeWidth = density * 1.4f)
    val barb = density * 6f
    val across = Offset(-along.y, along.x) / half
    val back = along / half
    drawLine(colors.muted, tip, tip - back * barb + across * (barb * .6f), strokeWidth = density * 1.4f)
    drawLine(colors.muted, tip, tip - back * barb - across * (barb * .6f), strokeWidth = density * 1.4f)
    val label = text.measure("wind from ${compassLabel(fromDegrees)}", TextStyle(color = colors.muted, fontSize = 11.sp))
    drawText(label, topLeft = Offset(density * 10f, middle.y + half + density * 6f))
}

/** The unit vector over the ground, east and north, that a wind coming from [fromDegrees] on the compass blows toward. */
internal fun windToward(fromDegrees: Float): Pair<Float, Float> {
    val bearing = fromDegrees * PI.toFloat() / 180f
    return -sin(bearing) to -cos(bearing)
}

/** Half the length of the wind arrow (dp). */
private const val WindArrow = 14f

internal fun DrawScope.drawListener(centre: Offset, heading: Float?, colors: PlayerColors, selected: Boolean) {
    if (heading != null) {
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
    if (selected) {
        drawCircle(colors.foreground, density * 11f, centre, style = Stroke(width = density * 1.6f))
        if (heading != null) {
            // The handle that turns the listener, like a sound's.
            val handle = facingHandle(centre, heading, FacingReach * density)
            drawLine(colors.foreground, centre, handle, strokeWidth = density * 1.4f)
            drawCircle(colors.backdrop, density * 6f, handle)
            drawCircle(colors.foreground, density * 6f, handle, style = Stroke(width = density * 1.6f))
        }
    }
}

/**
 * A loudspeaker: a small cabinet turned toward the listener, its name beside it. One the output
 * plays through is filled as far as its level; one it passes by is a faint outline.
 */
internal fun DrawScope.drawSpeaker(at: Offset, listener: Offset, name: String, inUse: Boolean, level: Float, colors: PlayerColors, text: TextMeasurer) {
    val toward = atan2(listener.y - at.y, listener.x - at.x)
    val half = density * SpeakerHalf
    val ink = if (inUse) colors.foreground else colors.hairline
    rotate(toward * 180f / PI.toFloat(), at) {
        val box = androidx.compose.ui.geometry.Rect(at.x - half, at.y - half, at.x + half * .6f, at.y + half)
        drawRect(colors.backdrop, box.topLeft, box.size)
        if (inUse && level > 0f) {
            drawRect(colors.foreground.copy(alpha = .25f + .6f * level), box.topLeft, box.size.copy(width = box.width * level))
        }
        drawRect(ink, box.topLeft, box.size, style = Stroke(width = density * 1.2f))
        // The cone, facing the listener.
        val cone = Path().apply {
            moveTo(at.x + half * .6f, at.y - half * .55f)
            lineTo(at.x + half * 1.2f, at.y - half)
            lineTo(at.x + half * 1.2f, at.y + half)
            lineTo(at.x + half * .6f, at.y + half * .55f)
        }
        drawPath(cone, ink, style = Stroke(width = density * 1.2f))
    }
    val label = text.measure(name, TextStyle(color = if (inUse) colors.muted else colors.hairline, fontSize = 11.sp))
    drawText(label, topLeft = at + Offset(-label.size.width / 2f, half + density * 4f))
}

/** Half a speaker's size on the map (dp). */
private const val SpeakerHalf = 7f

/**
 * An ambient sound is heard everywhere, so it has no circle of its own: broad
 * bands arriving from the side its anchor lies on, or rings all round when
 * the anchor is at the listener. The small mark is the anchor a drag moves.
 */
internal fun DrawScope.drawAmbient(marker: Marker, listenerAt: Point, view: TopDown, colors: PlayerColors, text: TextMeasurer, selected: Boolean, beat: Float) {
    val at = view.toScreen(marker.offset)
    val centre = view.toScreen(listenerAt)
    val fromListener = Point(marker.offset.x - listenerAt.x, marker.offset.y - listenerAt.y)
    val alpha = if (marker.silent) .10f else .22f + .30f * marker.strength
    val stroke = Stroke(width = density * 7f, cap = androidx.compose.ui.graphics.StrokeCap.Round)
    val from = atan2(fromListener.y, fromListener.x)
    val surround = !fromListener.hasGroundBearing() || hypot(fromListener.x, fromListener.y) < SurroundWithin
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
internal fun DrawScope.drawFacingHandle(at: Offset, marker: Marker, colors: PlayerColors) {
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
internal fun DrawScope.drawReach(at: Offset, marker: Marker, view: TopDown, beat: Float, selected: Boolean) {
    val radius = marker.reach * view.pixelsPerMetre
    val heard = !marker.silent && marker.sounding && marker.strength > 0f
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

internal fun DrawScope.drawPoint(at: Offset, marker: Marker, colors: PlayerColors, text: TextMeasurer, selected: Boolean) {
    // Its size: a soft disc around the dot, wider the wider it sounds.
    if (marker.size > 0f) drawCircle(marker.color.copy(alpha = if (marker.silent) .08f else .2f), density * (6f + WidestSize * marker.size), at)
    if (selected) drawCircle(colors.foreground, density * 11f, at, style = Stroke(width = density * 1.6f))
    if (marker.silent) {
        drawCircle(colors.backdrop, density * 6f, at)
        drawCircle(marker.color, density * 6f, at, style = Stroke(width = density * 1.5f))
    } else if (!marker.sounding) {
        // Resting between two takes: the dot dims until it sounds again.
        drawCircle(colors.backdrop, density * 6f, at)
        drawCircle(marker.color.copy(alpha = .35f), density * 6f, at)
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
internal fun DrawScope.drawBearingLine(centre: Offset, at: Offset, offset: Point, colors: PlayerColors, text: TextMeasurer) {
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

/** How far past its dot the disc of a sound that is all around reaches (dp). */
private const val WidestSize = 22f

private const val ListenerWedge = 40f
private const val ListenerSpread = (35.0 * PI / 180.0).toFloat()
private const val WedgeSteps = 12

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
