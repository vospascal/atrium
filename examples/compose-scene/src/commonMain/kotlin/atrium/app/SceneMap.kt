package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.sp
import atrium.engine.SceneDescription
import atrium.engine.SceneFile
import atrium.engine.Telemetry
import kotlin.math.abs
import kotlin.math.max

/** A pin on the map, relative to the listener (m). */
private data class Pin(val name: String, val offset: Point, val plume: Plume)

/**
 * The scene from above at an angle, centred on the listener: every sound as
 * a pin in its ink color, raised to its height, with a halo as strong as its
 * plume. Positions are live while the scene plays.
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
    val pins = scene.sounds.mapIndexedNotNull { slot, sound ->
        val plume = plumes.getOrNull(slot) ?: return@mapIndexedNotNull null
        val live = telemetry?.sources?.firstOrNull { it.slot.toInt() == slot }
        val listener = description?.listener
        val offset = if (live != null && listener != null) {
            Point(live.position.x - listener.x, live.position.y - listener.y, live.position.z - listener.z)
        } else {
            sound.offsetFromListener.toPoint()
        }
        Pin(sound.name, offset, if (tint == null) plume else plume.copy(color = tint))
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

/** A grid line every 5, 10 or 20 m, so a map shows about ten of them. */
private fun gridStep(half: Float) = when {
    half <= 25f -> 5f
    half <= 50f -> 10f
    else -> 20f
}
