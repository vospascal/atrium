package atrium.app.editor

import androidx.compose.ui.graphics.Color
import atrium.app.Point

/** A sound as the map draws it: where it is now, relative to the map's origin. */
internal data class Marker(
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
    /** How wide it sounds, 0 a point to 1 all around. */
    val size: Float,
    /** The arc an ambient sound comes from (degrees): 360 all round. */
    val arc: Float,
    val strength: Float,
    val silent: Boolean,
    /** Whether it is uttering something now; a voice with takes rests between them. */
    val sounding: Boolean,
) {
    /** A sound with a beam can be turned; an ambient or omni one sounds the same whichever way it faces. */
    val turnable: Boolean get() = !ambient && beamHalfAngle != null
}

/** A sound whose facing handle is being dragged: the map owns its heading until the drag ends. */
internal data class Turned(val slot: UShort, val heading: Float)

/**
 * The sounds being dragged, by slot: the map owns their places until the engine reports the one
 * under the pointer, [lead], where it was dropped. Each keeps its distance to the lead, so several
 * selected sounds move as one. While [copying], with Alt held, the sounds stay where they are
 * and the places are where their copies will land.
 */
internal data class Dragged(val lead: UShort, val start: Map<UShort, Point>, val offsets: Map<UShort, Point>, val copying: Boolean) {
    /** With the lead at [at] and every other sound as far from it as when the drag began. */
    fun movedTo(at: Point): Dragged {
        val from = start.getValue(lead)
        return copy(offsets = start.mapValues { (_, began) -> Point(began.x + at.x - from.x, began.y + at.y - from.y) })
    }
}

/** The listener while it is dragged or turned: the map owns its place and heading until the drag ends. */
internal data class HeldListener(val offset: Point, val heading: Float, val turning: Boolean)

/** How far from its sound the facing handle sits (dp). */
internal const val FacingReach = 46f
