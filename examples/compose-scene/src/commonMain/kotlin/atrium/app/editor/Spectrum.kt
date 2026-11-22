package atrium.app.editor

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import atrium.app.session.AnalysisHistory
import atrium.app.ui.imageOf
import kotlin.math.roundToInt

/**
 * The colour of spectrum value [value], 0 to 255, as 0xRRGGBB: the offline picture's map
 * (`crates/analysis/src/picture/spectrogram.rs`), matplotlib's "inferno" at nine points, dark
 * to bright through purple, red and yellow, so a live spectrum and a report's picture read alike
 * and brighter is always louder, in colour and in grey.
 */
internal fun spectrumColour(value: Int): Int {
    val fraction = value.coerceIn(0, 255) / 255.0
    val position = fraction * (InfernoStops.size - 1)
    val below = position.toInt().coerceAtMost(InfernoStops.size - 2)
    val share = position - below
    fun mix(channel: Int) = (InfernoStops[below][channel] * (1 - share) + InfernoStops[below + 1][channel] * share).roundToInt()
    return (mix(0) shl 16) or (mix(1) shl 8) or mix(2)
}

/** The offline picture's colour stops, copied from its `colour`, lowest first. */
private val InfernoStops = arrayOf(
    intArrayOf(0, 0, 4),
    intArrayOf(31, 12, 72),
    intArrayOf(85, 15, 109),
    intArrayOf(136, 34, 106),
    intArrayOf(186, 54, 85),
    intArrayOf(227, 89, 51),
    intArrayOf(249, 140, 10),
    intArrayOf(249, 201, 50),
    intArrayOf(252, 255, 164),
)

/** The colour map's stops as Compose colours, for the scale beside the strip. */
internal val SpectrumScaleColours: List<Color> = InfernoStops.map { Color(it[0], it[1], it[2]) }

private val ColourOfValue = IntArray(256) { spectrumColour(it) }

/**
 * The spectrum lane's picture of [history]: one column of pixels per analysis column, highest
 * row on top, built a tile at a time as columns arrive rather than from scratch every frame,
 * so ten minutes (12 000 columns) draw as a handful of images.
 *
 * When more columns fall on a pixel than one, a tile's pixel holds the loudest of each [pool]
 * columns in every row: a short call stays visible at the widest zoom, where sampling every
 * tenth column would lose most of them.
 */
internal class SpectrumPicture(private val history: AnalysisHistory) {
    private class Tile(val image: ImageBitmap, val pixels: ByteArray, val builtUpTo: Long)

    private val tiles = HashMap<Long, Tile>()
    private var generation = -1
    private var pool = 1

    /**
     * Draw the columns from [from] up to [until] into the band from [top] to [top] + [height],
     * each pooled group of columns at [xOf] its first column, [pooled] columns to a pixel.
     */
    fun draw(scope: DrawScope, from: Long, until: Long, pooled: Int, top: Float, height: Float, xOf: (Long) -> Float) {
        val rows = history.rows
        if (rows == 0 || until <= from) return
        if (generation != history.generation || pool != pooled) {
            tiles.clear()
            generation = history.generation
            pool = pooled
        }
        val columnsPerTile = TileWidth.toLong() * pool
        // Tiles wholly before the oldest column kept are of no use again.
        tiles.keys.filter { (it + 1) * columnsPerTile <= history.first }.forEach { tiles.remove(it) }
        val firstPooled = from / pool
        val lastPooled = (until - 1) / pool
        for (tileNumber in firstPooled / TileWidth..lastPooled / TileWidth) {
            val tile = tileFor(tileNumber, rows)
            val tileStart = tileNumber * TileWidth
            val startPooled = maxOf(firstPooled, tileStart)
            val endPooled = minOf(lastPooled + 1, tileStart + TileWidth)
            if (endPooled <= startPooled) continue
            // Rounded edges, so neighbouring tiles meet without a seam or an overlap.
            val left = xOf(startPooled * pool).roundToInt()
            val right = xOf(endPooled * pool).roundToInt()
            if (right <= left) continue
            scope.drawImage(
                tile.image,
                srcOffset = IntOffset((startPooled - tileStart).toInt(), 0),
                srcSize = IntSize((endPooled - startPooled).toInt(), rows),
                dstOffset = IntOffset(left, top.roundToInt()),
                dstSize = IntSize(right - left, height.roundToInt()),
                // Each column a crisp block, as the offline picture draws its cells.
                filterQuality = FilterQuality.None,
            )
        }
    }

    /**
     * Tile [tileNumber], brought up to date when columns have arrived in it since it was last
     * built: only the pooled columns those fall in are worked out again.
     */
    private fun tileFor(tileNumber: Long, rows: Int): Tile {
        val columnsPerTile = TileWidth.toLong() * pool
        val firstColumn = tileNumber * columnsPerTile
        val wanted = minOf(history.end, firstColumn + columnsPerTile)
        val built = tiles[tileNumber]
        if (built != null && built.builtUpTo >= wanted) return built
        val pixels = built?.pixels ?: ByteArray(TileWidth * rows * 4)
        val fromPooled = if (built == null) 0 else ((built.builtUpTo - firstColumn) / pool).toInt()
        val untilPooled = ((wanted - firstColumn + pool - 1) / pool).toInt()
        for (pooledColumn in fromPooled until untilPooled) {
            val start = firstColumn + pooledColumn.toLong() * pool
            for (row in 0 until rows) {
                var loudest = -1
                for (column in start until start + pool) {
                    if (history.has(column)) loudest = maxOf(loudest, history.spectrumAt(column, row))
                }
                // The highest row at the top of the picture; a column never delivered stays clear.
                val at = ((rows - 1 - row) * TileWidth + pooledColumn) * 4
                if (loudest < 0) {
                    pixels.fill(0, at, at + 4)
                    continue
                }
                val colour = ColourOfValue[loudest]
                pixels[at] = (colour shr 16).toByte()
                pixels[at + 1] = (colour shr 8).toByte()
                pixels[at + 2] = colour.toByte()
                pixels[at + 3] = 0xFF.toByte()
            }
        }
        return Tile(imageOf(TileWidth, rows, pixels), pixels, wanted).also { tiles[tileNumber] = it }
    }

    private companion object {
        /** Pooled columns a tile holds: small enough that rebuilding the newest one each frame is cheap. */
        const val TileWidth = 256
    }
}
