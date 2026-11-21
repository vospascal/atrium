package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.unit.dp
import atrium.engine.LoudnessMap
import atrium.engine.Position
import atrium.app.ui.Backdrop
import atrium.app.ui.Foreground
import atrium.app.ui.LocalPlayerColors
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.ui.PlayerColors
import atrium.app.Point

/**
 * The loudness layer of the editor's map: how loud the scene's direct sound is at every spot of
 * the ground, as bands of one hue. Each band is [LoudnessBandWidth] dB, which for one sound in
 * the open is a doubling of distance, so the bands read as rings. The loudest band ends at the
 * level of the loudest sound at 1 m; quiet recedes into the paper and loud stands out from it,
 * on light paper and on dark alike. Further down than the last band nothing is drawn, so the
 * ground beyond a sound's reach stays plain.
 *
 * It is a guide to reach: echoes, reverb, the ground and the air are left out, and the legend
 * says so.
 */
internal const val LoudnessBandWidth = 6f

/** One hue from near the paper to deep, lightest first: a sequential ramp, never a rainbow. */
private val LoudnessRamp = listOf(
    Color(0xFFCDE2FB), Color(0xFF9EC5F4), Color(0xFF6DA7EC), Color(0xFF3987E5), Color(0xFF256ABF), Color(0xFF184F95), Color(0xFF0D366B),
)

/** How many bands there are; the loudest also holds everything louder. */
internal val LoudnessBands = LoudnessRamp.size

/** Where the loudest band ends for a scene whose loudest sound is [loudest] dB at 1 m: the next whole band up. */
internal fun loudestBandTop(loudest: Float): Float = kotlin.math.ceil(loudest / LoudnessBandWidth) * LoudnessBandWidth

/**
 * The band a level of [db] dB SPL falls in when the loudest band ends at [top]: 0 the quietest
 * shown, or null when it is too quiet to draw.
 */
internal fun loudnessBand(db: Float, top: Float): Int? {
    if (db.isNaN() || db <= 0f) return null
    val below = kotlin.math.floor((top - db) / LoudnessBandWidth).toInt().coerceAtLeast(0)
    return (LoudnessBands - 1 - below).takeIf { it >= 0 }
}

/** The colour of [band] on a [dark] or light map: the quiet end is always the one nearest the paper. */
internal fun loudnessColor(band: Int, dark: Boolean): Color =
    if (dark) LoudnessRamp[LoudnessBands - 1 - band] else LoudnessRamp[band]

internal val PlayerColors.dark: Boolean get() = backdrop.luminance() < .5f

/** How many points the layer asks for along the wider side of the map. */
internal const val LoudnessPoints = 144

/** The patch of ground a map [size] across shows through [view], as two world corners at [height], and its points. */
internal data class LoudnessPatch(val from: Position, val to: Position, val columns: Int, val rows: Int)

internal fun loudnessPatch(view: TopDown, size: Size, origin: Position, height: Float): LoudnessPatch? {
    if (size.width < 1f || size.height < 1f) return null
    val southWest = view.toWorld(Offset(0f, size.height))
    val northEast = view.toWorld(Offset(size.width, 0f))
    val wider = maxOf(size.width, size.height)
    return LoudnessPatch(
        Position(origin.x + southWest.x, origin.y + southWest.y, height),
        Position(origin.x + northEast.x, origin.y + northEast.y, height),
        columns = (LoudnessPoints * size.width / wider).toInt().coerceAtLeast(1),
        rows = (LoudnessPoints * size.height / wider).toInt().coerceAtLeast(1),
    )
}

/** Draw [map] under everything else: one square per point, in the colour of its band below [top]. */
internal fun DrawScope.drawLoudness(map: LoudnessMap, top: Float, view: TopDown, origin: Position, dark: Boolean) {
    val columns = map.columns.toInt()
    if (columns == 0) return
    map.levelsDb.forEachIndexed { index, level ->
        val band = loudnessBand(level, top) ?: return@forEachIndexed
        val west = map.corner.x - origin.x + (index % columns) * map.cell
        val south = map.corner.y - origin.y + (index / columns) * map.cell
        // Snapped to whole pixels, so neighbours meet without a seam or an overlap.
        val from = view.toScreen(Point(west, south + map.cell))
        val to = view.toScreen(Point(west + map.cell, south))
        val left = kotlin.math.round(from.x)
        val up = kotlin.math.round(from.y)
        drawRect(loudnessColor(band, dark).copy(alpha = LoudnessAlpha), Offset(left, up), Size(kotlin.math.round(to.x) - left, kotlin.math.round(to.y) - up))
    }
}

/** See-through enough that rings, reaches and names stay readable on top. */
private const val LoudnessAlpha = .6f

/** What the bands mean: the ramp with the level each starts at below [top], what it adds up, and what it leaves out. */
@Composable
internal fun LoudnessLegend(of: String, top: Float, modifier: Modifier = Modifier) {
    val dark = LocalPlayerColors.current.dark
    Column(modifier.background(Backdrop.copy(alpha = .92f), RoundedCornerShape(6.dp)).padding(horizontal = 10.dp, vertical = 8.dp)) {
        PaperText("Loudness · $of", 12, Foreground, singleLine = true)
        Spacer(Modifier.height(5.dp))
        Row {
            repeat(LoudnessBands) { band ->
                Canvas(Modifier.width(LegendStep).height(8.dp)) { drawRect(loudnessColor(band, dark)) }
            }
        }
        Row {
            repeat(LoudnessBands) { band ->
                val from = (top - (LoudnessBands - band) * LoudnessBandWidth).toInt()
                PaperText(if (band == LoudnessBands - 1) "$from+" else "$from", 10, Muted, singleLine = true, modifier = Modifier.width(LegendStep))
            }
        }
        PaperText("dB · direct sound only, no reverb · a band is twice as far", 10, Muted, singleLine = true)
    }
}

private val LegendStep = 30.dp
