package atrium.app

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import kotlin.math.hypot
import kotlin.math.min

/**
 * The scene from straight above with north up and the listener in the middle.
 * [reach] metres fit between the middle and the nearer edge of [size], so
 * distances and angles on screen are the real ones, scaled.
 */
internal class TopDown(size: Size, reach: Float) {
    val pixelsPerMetre = (min(size.width, size.height) / 2f / reach).coerceAtLeast(.01f)
    private val middle = Offset(size.width / 2f, size.height / 2f)

    /** Where a point [offset] metres from the listener is drawn. North (+y) is up. */
    fun toScreen(offset: Point) = Offset(middle.x + offset.x * pixelsPerMetre, middle.y - offset.y * pixelsPerMetre)

    /** The point, in metres from the listener, drawn at [screen]. */
    fun toWorld(screen: Offset) = Point((screen.x - middle.x) / pixelsPerMetre, (middle.y - screen.y) / pixelsPerMetre)
}

/** The index of the point nearest to [at], if one lies within [radius] of it. */
internal fun nearestWithin(points: List<Offset>, at: Offset, radius: Float): Int? =
    points.withIndex()
        .map { (index, point) -> index to hypot(point.x - at.x, point.y - at.y) }
        .filter { it.second <= radius }
        .minByOrNull { it.second }
        ?.first

/**
 * How far from the listener the map has to show, in metres: past the farthest
 * of [offsets], and never so little that a lone nearby sound fills the view.
 */
internal fun reachFor(offsets: List<Point>): Float =
    maxOf(offsets.maxOfOrNull { hypot(it.x, it.y) } ?: 0f, NearestReach) * 1.2f

private const val NearestReach = 10f

/** The distance rings to draw inside [reach] metres: 5, 10, 20, 40, … */
internal fun ringsWithin(reach: Float): List<Float> =
    generateSequence(5f) { it * 2f }.takeWhile { it <= reach }.toList()
