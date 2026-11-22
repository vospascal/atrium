package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import atrium.engine.EmitterKind
import atrium.engine.SourceDescription
import atrium.engine.SourceTelemetry
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.ln
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sin
import atrium.app.ui.Chip
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ui.Muted
import atrium.app.ui.PaperLevel
import atrium.app.ui.PaperText
import atrium.app.Point
import atrium.app.session.Session
import atrium.app.bearingLabel

/**
 * How a sound fills the air: how wide it goes out and how much is heard behind
 * it. The named shapes are starting points on the same two values, the way a
 * preset is, and the drawing shows what they make.
 */
@Composable
internal fun ShapePart(session: Session, sound: SourceDescription, live: SourceTelemetry?) {
    val ambient = sound.emitterKind == EmitterKind.FIELD
    // Any sound is either a point with a place, or the air of the place itself.
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Chip("Point", selected = !ambient) { if (ambient) session.ambient(sound, false) }
        Chip("Ambient", selected = ambient) { if (!ambient) session.ambient(sound, true) }
    }
    Spacer(Modifier.height(10.dp))
    if (ambient) {
        PaperText(shapeLine(sound), 15)
        PaperText(if (sound.arc >= AllRound) "Arc: all round" else "Arc ${sound.arc.roundToInt()}°", 15)
        PaperLevel(
            (sound.arc - NarrowestArc) / (AllRound - NarrowestArc),
            onChange = { session.arc(sound.slot, NarrowestArc + it * (AllRound - NarrowestArc)) },
            // A degree an arrow press.
            step = 1f / (AllRound - NarrowestArc),
            modifier = Modifier.fillMaxWidth(),
        )
        PaperText(
            "It has no distance or echo: its level is what is heard wherever the listener stands. " +
                "Narrower than all round, it comes from the side its place lies on, as a shore or a tree line does.",
            14,
            Muted,
        )
        return
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        StartingShape.entries.forEach { shape ->
            Chip(shape.label, selected = shape.matches(sound.width, sound.behind)) {
                session.beam(sound.slot, shape.width, shape.behind)
            }
        }
    }
    Spacer(Modifier.height(12.dp))
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            PaperText("Width ${sound.width.roundToInt()}°", 15)
            PaperLevel(
                (sound.width - NarrowestBeam) / (AllRound - NarrowestBeam),
                onChange = { session.beam(sound.slot, NarrowestBeam + it * (AllRound - NarrowestBeam), sound.behind) },
                // A degree an arrow press.
                step = 1f / (AllRound - NarrowestBeam),
                modifier = Modifier.fillMaxWidth(),
            )
            if (sound.width < AllRound) {
                PaperText("Behind ${behindLabel(sound.behind)}", 15)
                PaperLevel(
                    (sound.behind - SilentBehind) / -SilentBehind,
                    onChange = { session.beam(sound.slot, sound.width, SilentBehind + it * -SilentBehind) },
                    // A dB an arrow press.
                    step = 1f / -SilentBehind,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
        Spacer(Modifier.width(14.dp))
        val facing = live?.facing ?: sound.facing
        Polar(sound.width, sound.behind, atan2(facing.y, facing.x))
    }
    if (sound.beamHalfAngle != null) {
        Spacer(Modifier.height(4.dp))
        PaperText("Facing ${bearingLabel(live?.facing ?: sound.facing)}. Turn it with the handle on the map.", 14, Muted)
    }
    Spacer(Modifier.height(8.dp))
    PaperText(if (sound.size <= 0f) "Size: a point" else "Size ${(sound.size * 100).roundToInt()} %", 15)
    PaperLevel(sound.size, onChange = { session.size(sound.slot, it) }, modifier = Modifier.fillMaxWidth())
    PaperText("A wider sound spreads over more loudspeakers and feeds less of the reverb; from 75 % none.", 14, Muted)
}

/** The shapes to start from. Each sets a width and how much is heard behind; both can be changed after. */
internal enum class StartingShape(val label: String, val width: Float, val behind: Float) {
    Omni("Omni", AllRound, SilentBehind),
    Focused("Focused", 180f, SilentBehind),
    Narrow("Narrow", 90f, -12f),
    ;

    fun matches(width: Float, behind: Float) =
        abs(width - this.width) < 1f && (width >= AllRound || abs(behind - this.behind) < 1f)
}

internal const val AllRound = 360f
private const val NarrowestBeam = 10f

/** The narrowest arc an ambient sound can come from (degrees): the engine's limit. */
internal const val NarrowestArc = 10f

/** At this level behind, or lower, nothing is heard behind (dB). The engine's limit. */
internal const val SilentBehind = -60f

private fun behindLabel(behind: Float) =
    if (behind <= SilentBehind) "nothing" else if (behind >= 0f) "as loud as ahead" else "${behind.roundToInt()} dB"

/** How a sound fills the air, in a line. */
internal fun shapeLine(sound: SourceDescription): String = when {
    sound.emitterKind == EmitterKind.FIELD && sound.arc >= AllRound -> "Ambient: heard everywhere"
    sound.emitterKind == EmitterKind.FIELD -> "Ambient: heard everywhere, from an arc ${sound.arc.roundToInt()}° wide"
    sound.width >= AllRound -> "All round: equal in every direction"
    else -> "A beam ${sound.width.roundToInt()}° wide, ${behindLabel(sound.behind)} behind"
}

/**
 * How loud a sound [widthDegrees] wide with [behindDb] behind it is at [angle]
 * radians off the way it faces, 0 to 1. It is the engine's beam, drawn here
 * only: halfway between front and back at the edge of the width.
 */
internal fun beamGain(widthDegrees: Float, behindDb: Float, angle: Float): Float {
    val halfWidth = (widthDegrees.coerceIn(1f, AllRound) / 2f) * PI.toFloat() / 180f
    val behind = if (behindDb <= SilentBehind) 0f else 10f.pow(behindDb.coerceAtMost(0f) / 20f)
    val atEdge = (1f + cos(halfWidth)) / 2f
    val sharpness = if (atEdge <= 1e-6f) 0f else ln(.5f) / ln(atEdge)
    val lobe = ((1f + cos(angle)) / 2f).coerceAtLeast(0f).pow(sharpness)
    return behind + (1f - behind) * lobe
}

/** The beam seen from above, turned the way the sound faces, north up. */
@Composable
private fun Polar(width: Float, behind: Float, facing: Float) {
    val ink = Foreground
    val faint = Hairline
    Canvas(Modifier.size(92.dp)) {
        val centre = Offset(size.width / 2, size.height / 2)
        val radius = size.minDimension / 2 - 2.dp.toPx()
        drawCircle(faint, radius, centre, style = Stroke(1.dp.toPx()))
        drawCircle(faint, radius / 2, centre, style = Stroke(1.dp.toPx()))
        val outline = Path()
        val steps = 96
        for (step in 0..steps) {
            val angle = step * 2f * PI.toFloat() / steps
            val reach = radius * beamGain(width, behind, angle)
            // The world's +y is up on screen.
            val direction = facing + angle
            val point = Offset(centre.x + cos(direction) * reach, centre.y - sin(direction) * reach)
            if (step == 0) outline.moveTo(point.x, point.y) else outline.lineTo(point.x, point.y)
        }
        outline.close()
        drawPath(outline, ink.copy(alpha = .12f))
        drawPath(outline, ink, style = Stroke(1.5.dp.toPx()))
        drawCircle(ink, 2.5.dp.toPx(), centre)
    }
}
