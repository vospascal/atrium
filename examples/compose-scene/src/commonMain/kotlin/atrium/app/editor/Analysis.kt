package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.PointerType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import atrium.app.session.AnalysisHistory
import atrium.app.session.AnalysisSummary
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.ln
import kotlin.math.roundToInt
import kotlin.math.sqrt

/**
 * What was heard, measured as it played, on the same time axis as the ruler above: a row of
 * numbers over the span shown, then the spectrum in dB SPL with its colour scale, the A-weighted
 * FAST level with L90 marked, onsets, width and the ear's level difference. Right of now stays
 * empty: only what has sounded can be measured. Hovering shows the time and the values under
 * the pointer.
 */
@Composable
internal fun AnalysisLanes(history: AnalysisHistory, now: Double, shown: Double, ahead: Double, modifier: Modifier = Modifier) {
    val span = shown + ahead
    val version = history.version
    val summary = remember(version, shown) { history.summary(now - shown, now) }
    val picture = remember(history) { SpectrumPicture(history) }
    var pointer by remember { mutableStateOf<Offset?>(null) }
    var canvasSize by remember { mutableStateOf(IntSize.Zero) }
    val density = LocalDensity.current
    val text = rememberTextMeasurer()
    val ink = Foreground
    val faint = Hairline
    val muted = Muted
    val layout = laneLayout(canvasSize, density)
    val readout = pointer?.let { readoutAt(it, layout, history, now, shown, span) }
    Column(modifier) {
        Row(
            Modifier.fillMaxWidth().height(SummaryHeight).padding(start = 14.dp, end = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (summary == null) {
                PaperText("Nothing heard yet", 11, Muted, singleLine = true)
            } else {
                PaperText(summaryLine(summary), 11, Foreground, singleLine = true)
            }
            Spacer(Modifier.weight(1f).width(12.dp))
            if (readout != null) PaperText(readout, 11, Muted, singleLine = true)
        }
        Canvas(
            Modifier.weight(1f).fillMaxWidth()
                .onSizeChanged { canvasSize = it }
                .pointerInput(Unit) {
                    awaitPointerEventScope {
                        while (true) {
                            val event = awaitPointerEvent()
                            val change = event.changes.firstOrNull()
                            pointer = when {
                                event.type == PointerEventType.Exit -> null
                                // A finger lifted shows nothing; a mouse released stays over the lanes.
                                event.type == PointerEventType.Release && change?.type == PointerType.Touch -> null
                                else -> change?.position
                            }
                        }
                    }
                },
        ) {
            // From the size drawn at, not the one last measured: on the first frame that is not known yet.
            val layout = laneLayout(IntSize(size.width.roundToInt(), size.height.roundToInt()), this)
            val step = (history.scale?.columnSeconds ?: .05f).toDouble()
            val windowStart = now - shown
            fun xOf(seconds: Double) = (layout.left + layout.width * ((seconds - windowStart) / span)).toFloat()
            val nowX = xOf(now)
            val from = maxOf(history.first, history.indexAt(windowStart) - 1)
            val until = history.end
            val visible = history.end > 0 && until > from
            val columnsPerPixel = span / step / layout.width.coerceAtLeast(1f)
            // Pooled by powers of two, so the picture's tiles are rebuilt only when the zoom changes.
            var pool = 1
            while (pool * 2 <= columnsPerPixel) pool *= 2

            drawLaneNames(layout, text, muted)

            // The spectrum, with its frequencies on the left and its colour scale right of now.
            drawRect(faint.copy(alpha = .5f), Offset(layout.left, layout.spectrumTop), Size(nowX - layout.left, layout.spectrumHeight))
            if (visible) {
                clipRect(layout.left, 0f, nowX, size.height) {
                    picture.draw(this, from, until, pool, layout.spectrumTop, layout.spectrumHeight) { xOf(history.timeOf(it)) }
                }
            }
            history.scale?.let { scale ->
                val edges = scale.rowEdgesHertz
                for ((hertz, label) in FrequencyMarks) {
                    val position = (ln(hertz) - ln(edges.first().toDouble())) / (ln(edges.last().toDouble()) - ln(edges.first().toDouble()))
                    val y = layout.spectrumTop + layout.spectrumHeight * (1 - position).toFloat()
                    drawTick(label, y, layout, text, muted)
                }
                drawColourScale(layout, scale.spectrumFloorDb, scale.spectrumTopDb, text, muted)
            }

            // The level: the column's loudest FAST reading, with L90 across.
            val levels = if (visible) levelRange(history, from, until) else DefaultLevels
            fun levelY(decibels: Float) = layout.levelTop + layout.levelHeight * (1 - ((decibels - levels.start) / (levels.endInclusive - levels.start)).coerceIn(0f, 1f))
            drawLine(faint, Offset(layout.left, layout.levelTop + layout.levelHeight), Offset(layout.right, layout.levelTop + layout.levelHeight), 1f)
            drawTick("${levels.endInclusive.roundToInt()}", layout.levelTop, layout, text, muted)
            drawTick("${levels.start.roundToInt()}", layout.levelTop + layout.levelHeight, layout, text, muted)
            if (visible) {
                clipRect(layout.left, 0f, nowX, size.height) {
                    drawSeries(history, from, until, { history.lafAt(it) }, ::levelY, { xOf(history.timeOf(it)) }, ink.copy(alpha = .85f))
                }
            }
            summary?.let {
                val y = levelY(it.l90Db)
                drawLine(muted, Offset(layout.left, y), Offset(nowX, y), 1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 3.dp.toPx())))
                val label = text.measure("L90 ${it.l90Db.roundToInt()}", TextStyle(fontSize = 10.sp, color = muted))
                drawText(label, topLeft = Offset((nowX + 4.dp.toPx()).coerceAtMost(layout.right - label.size.width), y - label.size.height / 2f))
            }

            // Onsets: a tick where something started.
            drawLine(faint, Offset(layout.left, layout.onsetsTop + layout.onsetsHeight), Offset(layout.right, layout.onsetsTop + layout.onsetsHeight), 1f)
            if (visible) {
                var lastPixel = Int.MIN_VALUE
                for (index in from until until) {
                    if (!history.has(index) || !history.onsetAt(index)) continue
                    val x = xOf(history.timeOf(index))
                    if (x < layout.left || x > nowX) continue
                    // At the widest zoom several onsets share a pixel; one tick shows them.
                    val pixel = floor(x).toInt()
                    if (pixel == lastPixel) continue
                    lastPixel = pixel
                    drawLine(ink.copy(alpha = .7f), Offset(x, layout.onsetsTop + 1.dp.toPx()), Offset(x, layout.onsetsTop + layout.onsetsHeight), 1.dp.toPx())
                }
            }

            // Width, 0 one source to 1 diffuse.
            fun widthY(width: Float) = layout.widthTop + layout.widthHeight * (1 - width.coerceIn(0f, 1f))
            drawLine(faint, Offset(layout.left, layout.widthTop + layout.widthHeight), Offset(layout.right, layout.widthTop + layout.widthHeight), 1f)
            drawTick("1", layout.widthTop, layout, text, muted)
            drawTick("0", layout.widthTop + layout.widthHeight, layout, text, muted)
            if (visible) {
                clipRect(layout.left, 0f, nowX, size.height) {
                    drawSeries(history, from, until, { history.widthAt(it) }, ::widthY, { xOf(history.timeOf(it)) }, ink.copy(alpha = .85f))
                }
            }

            // The ear: right minus left, right louder upwards.
            fun earY(decibels: Float) = layout.earTop + layout.earHeight * (.5f - (decibels / (2 * EarRange)).coerceIn(-.5f, .5f))
            drawLine(faint, Offset(layout.left, earY(0f)), Offset(layout.right, earY(0f)), 1f)
            // Marked half-way out, clear of the width lane's foot just above.
            drawTick("R", earY(EarRange / 2), layout, text, muted)
            drawTick("L", earY(-EarRange / 2), layout, text, muted)
            if (visible) {
                clipRect(layout.left, 0f, nowX, size.height) {
                    drawSeries(history, from, until, { history.ildAt(it) }, ::earY, { xOf(history.timeOf(it)) }, ink.copy(alpha = .85f))
                }
            }

            drawLine(ink.copy(alpha = .5f), Offset(nowX, 0f), Offset(nowX, size.height), 1.dp.toPx())
            pointer?.let {
                if (it.x in layout.left..minOf(nowX, layout.right)) {
                    drawLine(ink.copy(alpha = .35f), Offset(it.x, 0f), Offset(it.x, size.height), 1.dp.toPx())
                }
            }
        }
    }
}

/** Where each lane lies in the canvas (px): the strip from [left] to [right], lanes top to bottom. */
private class LaneLayout(
    val left: Float,
    val right: Float,
    val spectrumTop: Float,
    val spectrumHeight: Float,
    val levelTop: Float,
    val levelHeight: Float,
    val onsetsTop: Float,
    val onsetsHeight: Float,
    val widthTop: Float,
    val widthHeight: Float,
    val earTop: Float,
    val earHeight: Float,
) {
    val width get() = right - left
}

/**
 * The lanes in a canvas of [size]: the strip lines up with the ruler and the tracks, and the
 * spectrum takes whatever height the other lanes leave, so a taller panel shows it finer.
 */
private fun laneLayout(size: IntSize, density: Density): LaneLayout = with(density) {
    val gap = LaneGap.toPx()
    val fixed = LevelLane.toPx() + OnsetsLane.toPx() + WidthLane.toPx() + EarLane.toPx() + gap * 5
    val spectrumHeight = (size.height - fixed).coerceAtLeast(SmallestSpectrum.toPx())
    val spectrumTop = gap
    val levelTop = spectrumTop + spectrumHeight + gap
    val onsetsTop = levelTop + LevelLane.toPx() + gap
    val widthTop = onsetsTop + OnsetsLane.toPx() + gap
    val earTop = widthTop + WidthLane.toPx() + gap
    LaneLayout(
        left = HeaderWidth.toPx(),
        right = (size.width - 14.dp.toPx()).coerceAtLeast(HeaderWidth.toPx() + 1f),
        spectrumTop = spectrumTop,
        spectrumHeight = spectrumHeight,
        levelTop = levelTop,
        levelHeight = LevelLane.toPx(),
        onsetsTop = onsetsTop,
        onsetsHeight = OnsetsLane.toPx(),
        widthTop = widthTop,
        widthHeight = WidthLane.toPx(),
        earTop = earTop,
        earHeight = EarLane.toPx(),
    )
}

/** The lanes' names in the header column, as the condition lanes name theirs. */
private fun DrawScope.drawLaneNames(layout: LaneLayout, text: TextMeasurer, color: Color) {
    val names = listOf(
        "Spectrum" to layout.spectrumTop,
        "Level" to layout.levelTop,
        "Onsets" to layout.onsetsTop,
        "Width" to layout.widthTop,
        "Ear" to layout.earTop,
    )
    for ((name, top) in names) {
        val label = text.measure(name, TextStyle(fontSize = 11.sp, color = color))
        drawText(label, topLeft = Offset(36.dp.toPx(), top - 2.dp.toPx()))
    }
}

/** A value on a lane's axis, right-aligned against the strip at height [y], with a short tick. */
private fun DrawScope.drawTick(label: String, y: Float, layout: LaneLayout, text: TextMeasurer, color: Color) {
    val measured = text.measure(label, TextStyle(fontSize = 9.sp, color = color))
    val top = (y - measured.size.height / 2f).coerceIn(0f, size.height - measured.size.height)
    drawText(measured, topLeft = Offset(layout.left - 6.dp.toPx() - measured.size.width, top))
    drawLine(color, Offset(layout.left - 4.dp.toPx(), y), Offset(layout.left - 1.dp.toPx(), y), 1f)
}

/**
 * The spectrum's colour scale at the strip's right end, where the future would be: the colour
 * map from [floorDb] at the foot to [topDb] at the head, labelled in dB SPL.
 */
private fun DrawScope.drawColourScale(layout: LaneLayout, floorDb: Float, topDb: Float, text: TextMeasurer, color: Color) {
    val style = TextStyle(fontSize = 9.sp, color = color)
    val top = text.measure("${topDb.roundToInt()}", style)
    val middle = text.measure("dB", style)
    val foot = text.measure("${floorDb.roundToInt()}", style)
    val labelWidth = maxOf(top.size.width, middle.size.width, foot.size.width)
    val bar = 6.dp.toPx()
    val barLeft = layout.right - labelWidth - 3.dp.toPx() - bar
    drawRect(
        Brush.verticalGradient(SpectrumScaleColours.reversed(), startY = layout.spectrumTop, endY = layout.spectrumTop + layout.spectrumHeight),
        Offset(barLeft, layout.spectrumTop),
        Size(bar, layout.spectrumHeight),
    )
    val labelLeft = barLeft + bar + 3.dp.toPx()
    drawText(top, topLeft = Offset(labelLeft, layout.spectrumTop))
    drawText(middle, topLeft = Offset(labelLeft, layout.spectrumTop + (layout.spectrumHeight - middle.size.height) / 2f))
    drawText(foot, topLeft = Offset(labelLeft, layout.spectrumTop + layout.spectrumHeight - foot.size.height))
}

/**
 * A measure over time, from column [from] up to [until], as a line where the columns are
 * fewer than the pixels. Where many fall on a pixel it is a band from the lowest to the highest
 * of them, so no peak is lost at the widest zoom; a filled band also costs far less to draw than
 * a stroke zigzagging through thousands of points. A column without a value (NaN, or never
 * delivered) breaks the line.
 */
private inline fun DrawScope.drawSeries(
    history: AnalysisHistory,
    from: Long,
    until: Long,
    valueOf: (Long) -> Float,
    yOf: (Float) -> Float,
    xOf: (Long) -> Float,
    color: Color,
) {
    // Inlined, so the measures and positions are read without boxing 12 000 of them a lane.
    // One entry a pixel: where it is, and the highest and lowest value in it (NaN for none).
    val capacity = (until - from).toInt().coerceAtMost(size.width.toInt() + 2).coerceAtLeast(1)
    val xs = FloatArray(capacity)
    val highs = FloatArray(capacity) { Float.NaN }
    val lows = FloatArray(capacity) { Float.NaN }
    var count = 0
    var pixel = Int.MIN_VALUE
    var columns = 0
    for (index in from until until) {
        val x = xOf(index)
        val at = floor(x).toInt()
        if (at != pixel) {
            if (count == capacity) break
            pixel = at
            xs[count++] = x
        }
        columns++
        if (!history.has(index)) continue
        val value = valueOf(index)
        if (value.isNaN()) continue
        val entry = count - 1
        if (highs[entry].isNaN() || value > highs[entry]) highs[entry] = value
        if (lows[entry].isNaN() || value < lows[entry]) lows[entry] = value
    }
    if (count == 0) return
    val path = Path()
    if (columns < count * 2) {
        var joined = false
        for (entry in 0 until count) {
            if (highs[entry].isNaN()) {
                joined = false
                continue
            }
            val y = yOf(highs[entry])
            if (joined) path.lineTo(xs[entry], y) else path.moveTo(xs[entry], y)
            joined = true
        }
        drawPath(path, color, style = Stroke(1.dp.toPx()))
        return
    }
    val thinnest = 1.dp.toPx()
    var entry = 0
    while (entry < count) {
        if (highs[entry].isNaN()) {
            entry++
            continue
        }
        // A run of pixels with values: along the highs, then back along the lows.
        var last = entry
        while (last + 1 < count && !highs[last + 1].isNaN()) last++
        path.moveTo(xs[entry], yOf(highs[entry]))
        for (along in entry + 1..last) path.lineTo(xs[along], yOf(highs[along]))
        val endX = if (last + 1 < count) xs[last + 1] else xs[last] + 1f
        path.lineTo(endX, yOf(highs[last]))
        path.lineTo(endX, maxOf(yOf(lows[last]), yOf(highs[last]) + thinnest))
        for (along in last downTo entry) path.lineTo(xs[along], maxOf(yOf(lows[along]), yOf(highs[along]) + thinnest))
        path.close()
        entry = last + 1
    }
    drawPath(path, color)
}

/**
 * The level lane's range (dB SPL) for the columns from [from] up to [until]: whole tens around
 * the quietest and loudest FAST readings, never narrower than [SmallestLevelRange] dB, so a
 * steady bed does not fill the lane with its smallest wobble.
 */
private fun levelRange(history: AnalysisHistory, from: Long, until: Long): ClosedFloatingPointRange<Float> {
    var lowest = Float.POSITIVE_INFINITY
    var highest = Float.NEGATIVE_INFINITY
    for (index in from until until) {
        if (!history.has(index)) continue
        val level = history.lafAt(index)
        // Digital silence reads −200 dB; it belongs at the foot, not in the range.
        if (level < QuietestLevel) continue
        lowest = minOf(lowest, level)
        highest = maxOf(highest, level)
    }
    if (lowest > highest) return DefaultLevels
    var low = floor(lowest / 10f) * 10f
    val high = maxOf(ceil(highest / 10f) * 10f, low + 10f)
    if (high - low < SmallestLevelRange) low = high - SmallestLevelRange
    return low..high
}

/** "LAeq 57 dB SPL · L90 40 · L10 62 · width 0.69 · onsets 44/min". */
internal fun summaryLine(summary: AnalysisSummary): String = buildList {
    add("LAeq ${summary.leqDb.roundToInt()} dB SPL")
    add("L90 ${summary.l90Db.roundToInt()}")
    add("L10 ${summary.l10Db.roundToInt()}")
    summary.width?.let { add("width ${twoDecimals(it)}") }
    add("onsets ${summary.onsetsPerMinute.roundToInt()}/min")
}.joinToString("  ·  ")

/**
 * What lies under [pointer]: how long ago, the level, width and ear there, and over the
 * spectrum the band and its level. Nothing right of now, where nothing has sounded.
 */
private fun readoutAt(pointer: Offset, layout: LaneLayout, history: AnalysisHistory, now: Double, shown: Double, span: Double): String? {
    if (pointer.x < layout.left || pointer.x > layout.right) return null
    val seconds = now - shown + (pointer.x - layout.left) / layout.width * span
    if (seconds > now) return null
    val parts = mutableListOf(agoLabel(now - seconds))
    val index = history.indexAt(seconds)
    if (!history.has(index)) return parts.single()
    parts += "LAF ${oneDecimal(history.lafAt(index))} dB"
    parts += "Leq ${oneDecimal(history.leqAt(index))} dB"
    history.widthAt(index).takeUnless { it.isNaN() }?.let { parts += "width ${twoDecimals(it)}" }
    history.ildAt(index).takeUnless { it.isNaN() }?.let { parts += "ear ${earLabel(it)}" }
    if (history.onsetAt(index)) parts += "onset"
    val edges = history.scale?.rowEdgesHertz
    if (edges != null && pointer.y in layout.spectrumTop..(layout.spectrumTop + layout.spectrumHeight)) {
        val rows = edges.size - 1
        val row = (rows - 1 - floor((pointer.y - layout.spectrumTop) / layout.spectrumHeight * rows).toInt()).coerceIn(0, rows - 1)
        val hertz = sqrt(edges[row].toDouble() * edges[row + 1].toDouble())
        parts += "${hertzLabel(hertz)} ${history.spectrumDecibels(history.spectrumAt(index, row)).roundToInt()} dB SPL"
    }
    return parts.joinToString("  ·  ")
}

/** "−12.3 s" or "−1:02.3": how long ago a moment was, as the ruler counts back from now. */
internal fun agoLabel(seconds: Double): String {
    val tenths = (seconds * 10).roundToInt().coerceAtLeast(0)
    val whole = tenths / 10
    return if (whole < 60) {
        "−$whole.${tenths % 10} s"
    } else {
        "−${whole / 60}:${(whole % 60).toString().padStart(2, '0')}.${tenths % 10}"
    }
}

/** "R 1.2 dB", "L 3.0 dB", or "even": which ear is louder, and by how much. */
private fun earLabel(decibels: Float): String = when {
    abs(decibels) < .05f -> "even"
    decibels > 0 -> "R ${oneDecimal(decibels)} dB"
    else -> "L ${oneDecimal(-decibels)} dB"
}

/** "450 Hz", "4.3 kHz". */
private fun hertzLabel(hertz: Double): String =
    if (hertz < 1000) "${hertz.roundToInt()} Hz" else "${oneDecimal((hertz / 1000).toFloat())} kHz"

/** A number with two decimals, without a platform formatter. */
private fun twoDecimals(value: Float): String {
    val hundredths = (value * 100).roundToInt()
    val sign = if (hundredths < 0) "-" else ""
    return "$sign${abs(hundredths) / 100}.${(abs(hundredths) % 100).toString().padStart(2, '0')}"
}

/** The frequencies marked beside the spectrum. */
private val FrequencyMarks = listOf(100.0 to "100", 1000.0 to "1k", 10000.0 to "10k")

/** How far either way the ear lane reaches (dB): past it the line stays at the edge. */
private const val EarRange = 12f

/** The level lane's range with nothing to show, and its narrowest (dB SPL). */
private val DefaultLevels = 30f..70f
private const val SmallestLevelRange = 30f
private const val QuietestLevel = -100f

private val SummaryHeight = 22.dp
private val LaneGap = 6.dp
private val LevelLane = 44.dp
private val OnsetsLane = 12.dp
private val WidthLane = 26.dp
private val EarLane = 26.dp
private val SmallestSpectrum = 48.dp

