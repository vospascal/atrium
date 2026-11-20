package atrium.app

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.pow

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
 * What a pointer at [at] picks: the nearest point within [radius], else the
 * nearest of the washes it lies inside. A wash is an ambient sound's whole
 * drawn area, so it can be grabbed anywhere, but a point on top of it wins.
 */
internal fun pickAt(points: List<Pair<Int, Offset>>, washes: List<Pair<Int, Offset>>, washRadius: Float, at: Offset, radius: Float): Int? =
    nearestWithin(points.map { it.second }, at, radius)?.let { points[it].first }
        ?: nearestWithin(washes.map { it.second }, at, washRadius)?.let { washes[it].first }

/** Where the handle that turns a sound sits: [reach] pixels from the sound at [at], the way it faces. North is up. */
internal fun facingHandle(at: Offset, heading: Float, reach: Float) =
    Offset(at.x + kotlin.math.cos(heading) * reach, at.y - kotlin.math.sin(heading) * reach)

/** The direction over the ground from a sound drawn at [from] to a pointer at [to]: east and north, in pixels. */
internal fun groundDirection(from: Offset, to: Offset) = Point(to.x - from.x, from.y - to.y)

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

/**
 * How far a sound reaches, in metres: where its level, falling 6 dB for every
 * doubling of distance from [referenceDistance], is down to [FaintLevel]. It is
 * distance alone, with no air or ground in it, so it compares sounds; the
 * engine decides what is heard.
 */
internal fun reachOf(level: Float, referenceDistance: Float): Float =
    (referenceDistance * 10f.pow((level - FaintLevel) / 20f)).coerceIn(ShortestReach, LongestReach)

/** The level a sound has fallen to at the edge of its drawn reach (dB SPL): ordinary talk. */
internal const val FaintLevel = 60f
private const val ShortestReach = 1f
private const val LongestReach = 200f
