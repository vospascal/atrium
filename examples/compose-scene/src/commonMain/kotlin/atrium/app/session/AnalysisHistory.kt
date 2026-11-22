package atrium.app.session

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import atrium.engine.AnalysisColumn
import atrium.engine.AnalysisScale
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.log10
import kotlin.math.pow
import kotlin.math.roundToLong

/**
 * What the output sounded like over the last [window] seconds, a column every 50 ms as the
 * engine measured it: the analysis lanes under the timeline draw from it.
 *
 * Columns are kept by their place on the scene's clock, column n at [timeOf] n, so a column is
 * found by its time without a search, and the spectrum's picture can be built a tile at a time.
 * A column the engine did not deliver (its tap dropped a block) leaves a gap; one that does not
 * come after the newest (the clock stood still while paused) is not kept, since nothing new
 * sounded at that time.
 */
@Stable
internal class AnalysisHistory(private val window: Double = TrackMemory) {
    /** How to read the columns, from the engine that measured them; null before an engine starts. */
    var scale: AnalysisScale? = null
        private set

    /** Bumped on every change, so a drawing that reads this history knows to draw again. */
    var version by mutableStateOf(0)
        private set

    /** Bumped when everything is forgotten, so a picture built from the old columns is thrown away. */
    var generation = 0
        private set

    /** The number of the oldest column kept, and one past the newest. */
    var first = 0L
        private set
    var end = 0L
        private set

    /** The spectrum's rows, lowest first. */
    var rows = 0
        private set

    private var step = .05
    private var capacity = 0
    /** The scene time of column 0: the first column after the last restart or seek. */
    private var origin = Double.NaN

    private var present = BooleanArray(0)
    private var spectrum = ByteArray(0)
    private var leq = FloatArray(0)
    private var laf = FloatArray(0)
    private var onset = BooleanArray(0)
    private var width = FloatArray(0)
    private var ild = FloatArray(0)

    /** Start again for an engine whose columns read as [next] says. */
    fun restart(next: AnalysisScale) {
        scale = next
        step = next.columnSeconds.toDouble()
        rows = next.rowEdgesHertz.size - 1
        capacity = ceil(window / step).toInt() + 1
        present = BooleanArray(capacity)
        spectrum = ByteArray(capacity * rows)
        leq = FloatArray(capacity)
        laf = FloatArray(capacity)
        onset = BooleanArray(capacity)
        width = FloatArray(capacity)
        ild = FloatArray(capacity)
        clear()
    }

    /** Forget every column: the scene has moved, and what was heard belongs to the old moment. */
    fun clear() {
        origin = Double.NaN
        first = 0L
        end = 0L
        present.fill(false)
        generation++
        version++
    }

    /** Take in [columns], oldest first, as the engine returns them. */
    fun record(columns: List<AnalysisColumn>) {
        if (capacity == 0 || columns.isEmpty()) return
        for (column in columns) {
            if (origin.isNaN()) origin = column.sceneSeconds
            val index = ((column.sceneSeconds - origin) / step).roundToLong()
            if (index < end) continue
            if (index - end >= capacity) {
                // A gap longer than the window: nothing kept is still in it.
                present.fill(false)
            } else {
                for (missing in end until index) present[slot(missing)] = false
            }
            write(slot(index), column)
            end = index + 1
            first = maxOf(first, end - capacity, 0L)
            // What the window no longer covers goes, by time and not by count, as the tracks do.
            while (first < end && timeOf(first) < timeOf(end - 1) - window) first++
        }
        version++
    }

    /** When column [index] started, in scene seconds. */
    fun timeOf(index: Long): Double = origin + index * step

    /** The number of the column sounding at [seconds], kept or not. */
    fun indexAt(seconds: Double): Long = floor((seconds - origin) / step).toLong()

    /** Whether column [index] is kept, and was delivered. */
    fun has(index: Long): Boolean = index in first until end && present[slot(index)]

    /** Column [index]'s spectrum value in [row], 0 to 255; only for a column [has] says is there. */
    fun spectrumAt(index: Long, row: Int): Int = spectrum[slot(index) * rows + row].toInt() and 0xFF

    fun leqAt(index: Long): Float = leq[slot(index)]
    fun lafAt(index: Long): Float = laf[slot(index)]
    fun onsetAt(index: Long): Boolean = onset[slot(index)]

    /** The width and the ILD at [index]; NaN where the engine measured none. */
    fun widthAt(index: Long): Float = width[slot(index)]
    fun ildAt(index: Long): Float = ild[slot(index)]

    /** The band level (dB SPL) a spectrum value of [value] stands for. */
    fun spectrumDecibels(value: Int): Float {
        val reading = scale ?: return 0f
        return reading.spectrumFloorDb + value / 255f * (reading.spectrumTopDb - reading.spectrumFloorDb)
    }

    /**
     * The header's numbers over the columns that started from [from] to [to] seconds: null
     * when none of them is kept.
     */
    fun summary(from: Double, to: Double): AnalysisSummary? {
        if (end == 0L) return null
        // A hair of slack, so a span that starts exactly on a column is not lost to rounding.
        val start = maxOf(first, ceil((from - origin) / step - Slack).toLong())
        val stop = minOf(end, floor((to - origin) / step + Slack).toLong() + 1)
        if (stop <= start) return null
        // Plain arrays, not lists of boxed floats: ten minutes are 12 000 columns, worked out
        // again every time a column arrives.
        val equivalent = FloatArray((stop - start).toInt())
        val fast = FloatArray(equivalent.size)
        var count = 0
        var widthSum = 0.0
        var widths = 0
        var onsets = 0
        for (index in start until stop) {
            if (!has(index)) continue
            equivalent[count] = leqAt(index)
            fast[count] = lafAt(index)
            count++
            val width = widthAt(index)
            if (!width.isNaN()) {
                widthSum += width
                widths++
            }
            if (onsetAt(index)) onsets++
        }
        if (count == 0) return null
        val sorted = fast.copyOf(count).apply { sort() }
        val minutes = count * step / 60.0
        return AnalysisSummary(
            leqDb = energyMean(equivalent.copyOf(count)),
            l10Db = exceededSorted(sorted, 10.0),
            l90Db = exceededSorted(sorted, 90.0),
            width = if (widths == 0) null else (widthSum / widths).toFloat(),
            onsetsPerMinute = (onsets / minutes).toFloat(),
        )
    }

    private fun slot(index: Long): Int = (index % capacity).toInt()

    private companion object {
        /** How close to a column's start counts as on it, in columns. */
        const val Slack = 1e-6
    }

    private fun write(at: Int, column: AnalysisColumn) {
        present[at] = true
        val count = minOf(rows, column.spectrum.size)
        column.spectrum.copyInto(spectrum, at * rows, 0, count)
        if (count < rows) spectrum.fill(0, at * rows + count, at * rows + rows)
        leq[at] = column.leqOverallDb
        laf[at] = column.lafOverallDb
        onset[at] = column.onset
        width[at] = column.width ?: Float.NaN
        ild[at] = column.ildDb ?: Float.NaN
    }
}

/**
 * The numbers over the span the panel shows, as an `analyse` report gives them for a whole
 * render: [leqDb] the energy mean of the columns' A-weighted Leq; [l10Db] and [l90Db] the
 * FAST levels exceeded 10 % and 90 % of the time; the mean [width]; onsets a minute.
 */
internal data class AnalysisSummary(
    val leqDb: Float,
    val l10Db: Float,
    val l90Db: Float,
    val width: Float?,
    val onsetsPerMinute: Float,
)

/** The level of the mean power of [levels] (dB): a loud moment counts for what it carries, not for its decibels. */
internal fun energyMean(levels: FloatArray): Float =
    (10.0 * log10(levels.sumOf { 10.0.pow(it / 10.0) } / levels.size)).toFloat()

/**
 * The level [levels] exceed [percent] of the time (dB), as the analyser reports L10 and L90:
 * the (100 − [percent]) % quantile, interpolated between neighbours in power, not in decibels.
 */
internal fun exceeded(levels: FloatArray, percent: Double): Float = exceededSorted(levels.sortedArray(), percent)

private fun exceededSorted(sorted: FloatArray, percent: Double): Float {
    val position = (1.0 - percent / 100.0) * (sorted.size - 1)
    val below = floor(position).toInt()
    val above = ceil(position).toInt()
    val share = position - below
    val power = 10.0.pow(sorted[below] / 10.0) * (1.0 - share) + 10.0.pow(sorted[above] / 10.0) * share
    return (10.0 * log10(power)).toFloat()
}
