package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.sp
import atrium.engine.HeadTrackingStatus
import atrium.engine.Position
import atrium.engine.RenderMode
import atrium.engine.SceneDescription
import atrium.engine.SceneFile
import atrium.engine.Telemetry
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.roundToInt
import kotlin.math.sin

/** A pin on the map, relative to the listener (m). */
private data class Pin(val name: String, val offset: Point, val plume: Plume)

/** How far the listener's facing wedge reaches (dp), and how wide it opens to each side. */
private const val FacingReach = 56f
private const val FacingSpread = (35.0 * PI / 180.0).toFloat()
private const val FacingSteps = 12

/**
 * The scene from above at an angle, centred on the listener: every sound as
 * a pin in its ink color, raised to its height, with a halo as strong as its
 * plume. Positions are live while the scene plays.
 *
 * The wedge at the centre is where the listener faces, as the engine reports
 * it. It turns with the head, and darkens, while a head tracker is sending.
 */
@Composable
internal fun SceneMap(
    scene: SceneFile,
    description: SceneDescription?,
    telemetry: Telemetry?,
    plumes: List<Plume>,
    modifier: Modifier = Modifier,
) {
    val colors = LocalPlayerColors.current
    val tint = LocalInkTint.current
    val text = rememberTextMeasurer()
    // One pin per plume, so a sound added while the scene plays is on the map too.
    val pins = plumes.mapIndexedNotNull { slot, plume ->
        val sound = scene.sounds.getOrNull(slot)
        val built = description?.sources?.getOrNull(slot)
        val live = telemetry?.sources?.firstOrNull { it.slot.toInt() == slot }
        val listener = description?.listener
        val at = live?.position ?: built?.position
        val offset = when {
            at != null && listener != null -> Point(at.x - listener.x, at.y - listener.y, at.z - listener.z)
            sound != null -> sound.offsetFromListener.toPoint()
            else -> return@mapIndexedNotNull null
        }
        val name = built?.name ?: sound?.name ?: return@mapIndexedNotNull null
        Pin(name, offset, if (tint == null) plume else plume.copy(color = tint))
    }
    // Half the side of the square shown, with room around the farthest sound.
    val half = (pins.maxOfOrNull { max(abs(it.offset.x), abs(it.offset.y)) } ?: 10f) * 1.2f + 4f

    Canvas(modifier) {
        val projection = Projection(size, half * 2, half * 2)
        fun at(x: Float, y: Float, z: Float = 0f) = projection.project(Point(x + half, y + half, z))

        val step = gridStep(half)
        var line = -half - (-half).mod(step)
        while (line <= half) {
            drawLine(colors.hairline, at(line, -half), at(line, half), strokeWidth = density * .7f)
            drawLine(colors.hairline, at(-half, line), at(half, line), strokeWidth = density * .7f)
            line += step
        }

        val listener = at(0f, 0f)
        for (ring in 1..3) {
            drawCircle(colors.ring, density * 5f * ring, listener, style = Stroke(width = density * .7f))
        }

        telemetry?.let { live ->
            // The same size on screen for a small scene and a large one.
            val reach = density * FacingReach / projection.pixelsPerMetre
            val heading = atan2(live.listenerFacing.y, live.listenerFacing.x)
            val wedge = Path().apply {
                moveTo(listener.x, listener.y)
                for (step in 0..FacingSteps) {
                    val angle = heading - FacingSpread + 2 * FacingSpread * step / FacingSteps
                    val edge = at(cos(angle) * reach, sin(angle) * reach)
                    lineTo(edge.x, edge.y)
                }
                close()
            }
            val tracked = live.headTracking == HeadTrackingStatus.RECEIVING
            drawPath(wedge, colors.foreground.copy(alpha = if (tracked) .24f else .12f))
            drawLine(
                colors.foreground.copy(alpha = if (tracked) .9f else .45f),
                listener,
                at(cos(heading) * reach, sin(heading) * reach),
                strokeWidth = density * 1.4f,
            )
            headTrackingNote(live)?.let { note ->
                val label = text.measure(note, TextStyle(color = colors.muted, fontSize = 12.sp))
                // Along the bottom edge, clear of the pins and their names.
                drawText(
                    label,
                    topLeft = Offset((size.width - label.size.width) / 2f, size.height - label.size.height - density * 6f),
                )
            }
        }

        pins.sortedByDescending { it.offset.x + it.offset.y }.forEach { pin ->
            val ground = at(pin.offset.x, pin.offset.y)
            val height = at(pin.offset.x, pin.offset.y, max(0f, pin.offset.z))
            val halo = density * (18f + 30f * pin.plume.strength)
            drawCircle(
                Brush.radialGradient(
                    0f to pin.plume.color.copy(alpha = .45f * pin.plume.strength + .1f),
                    1f to pin.plume.color.copy(alpha = 0f),
                    center = ground,
                    radius = halo,
                ),
                halo,
                ground,
            )
            if (height != ground) drawLine(pin.plume.color, ground, height, strokeWidth = density * 1.2f)
            drawCircle(pin.plume.color, density * 5f, height)
            val label = text.measure(pin.name, TextStyle(color = colors.foreground, fontSize = 13.sp))
            drawText(label, topLeft = height + Offset(density * 9f, -label.size.height / 2f))
        }
    }
}

/**
 * What the map says about head tracking, or null when there is nothing to say:
 * speakers do not follow a head, so the hint to switch it on is for headphones only.
 */
internal fun headTrackingNote(live: Telemetry): String? = when (live.headTracking) {
    HeadTrackingStatus.RECEIVING -> "Head tracking · facing ${bearingLabel(live.listenerFacing)}"
    HeadTrackingStatus.WAITING -> "Head tracking · waiting for head movement"
    HeadTrackingStatus.OFF ->
        if (live.output.renderMode == RenderMode.HRTF) "Head tracking is off · switch it on in Settings" else null
}

/**
 * A facing as a compass bearing, the way the scene files give directions:
 * degrees clockwise from north (+Y), with the nearest of the eight compass points.
 */
internal fun bearingLabel(facing: Position): String {
    val degrees = ((atan2(facing.x, facing.y) * 180.0 / PI).roundToInt() + 360) % 360
    val point = CompassPoints[((degrees + 22) / 45) % 8]
    return "$degrees° $point"
}

private val CompassPoints = listOf("N", "NE", "E", "SE", "S", "SW", "W", "NW")

/** A grid line every 5, 10 or 20 m, so a map shows about ten of them. */
private fun gridStep(half: Float) = when {
    half <= 25f -> 5f
    half <= 50f -> 10f
    else -> 20f
}

/** An isometric-style view of a width × depth ground area, fitted to [size]. */
internal class Projection(size: Size, width: Float, depth: Float) {
    private val diagonals = (width + depth) * Diagonal
    val pixelsPerMetre = (minOf(size.width * .84f, size.height * 1.36f) / diagonals).coerceAtLeast(.01f)
    private val unit = pixelsPerMetre * Diagonal
    private val origin = Offset(
        size.width / 2 - (width - depth) / 2 * unit,
        size.height * .48f - (width + depth) / 2 * unit * Tilt,
    )

    fun project(point: Point) = origin + Offset(
        (point.x - point.y) * unit,
        (point.x + point.y) * unit * Tilt - point.z * pixelsPerMetre,
    )


    companion object {
        private const val Diagonal = .70710677f
        const val Tilt = .52f
    }
}
