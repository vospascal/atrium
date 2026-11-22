package atrium.app.session

import atrium.app.editor.agoLabel
import atrium.app.editor.spectrumColour
import atrium.app.editor.summaryLine
import atrium.engine.AnalysisColumn
import atrium.engine.AnalysisScale
import kotlin.math.abs
import kotlin.math.log10
import kotlin.math.pow
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class AnalysisHistoryTest {
    private val engineScale = AnalysisScale(0f, 90f, List(97) { 20f * 1000f.pow(it / 96f) }, .05f)

    private fun column(
        seconds: Double,
        leq: Float = 50f,
        laf: Float = leq,
        onset: Boolean = false,
        width: Float? = null,
        ild: Float? = null,
        spectrum: Int = 0,
    ) = AnalysisColumn(seconds, ByteArray(96) { spectrum.toByte() }, listOf(leq, leq), listOf(laf, laf), leq, laf, onset, width, ild)

    private fun history(window: Double = TrackMemory) = AnalysisHistory(window).apply { restart(engineScale) }

    private fun close(expected: Float, actual: Float, tolerance: Float = .005f, what: String) =
        assertTrue(abs(expected - actual) <= tolerance, "$what: expected $expected, got $actual")

    /** Ten minutes are kept, as the tracks keep them: 20 columns a second, the oldest let go. */
    @Test
    fun tenMinutesAreKeptAndTheOldestGo() {
        val history = history()
        // Eleven minutes at 50 ms a column.
        history.record(List(13_200) { column(it * .05) })
        assertEquals(13_200L, history.end)
        val kept = history.end - history.first
        assertTrue(kept in 12_000L..12_001L, "ten minutes and one column, got $kept")
        close(60f, history.timeOf(history.first).toFloat(), .051f, "the oldest is ten minutes back")
        assertFalse(history.has(history.first - 1))
        assertTrue(history.has(history.end - 1))
    }

    /** A seek forgets everything; the next columns, at the new time, start afresh there. */
    @Test
    fun aSeekStartsAfresh() {
        val history = history()
        history.record(List(100) { column(it * .05, leq = 70f) })
        val generation = history.generation
        history.clear()
        assertNull(history.summary(0.0, 10.0), "nothing is kept after a seek")
        assertTrue(history.generation > generation, "the spectrum's picture is thrown away")
        history.record(List(20) { column(120.0 + it * .05, leq = 40f) })
        assertEquals(0L, history.first)
        assertEquals(20L, history.end)
        assertEquals(120.0, history.timeOf(0))
        close(40f, history.summary(119.0, 122.0)!!.leqDb, what = "only the new columns count")
    }

    /** While paused the clock stands still: columns at a time already kept are left out; a dropped block leaves a gap. */
    @Test
    fun pausesAddNothingAndDropsLeaveGaps() {
        val history = history()
        history.record(List(10) { column(it * .05) })
        history.record(List(5) { column(.45) })
        assertEquals(10L, history.end, "a stopped clock adds nothing")
        history.record(listOf(column(1.0)))
        assertEquals(21L, history.end)
        assertFalse(history.has(15), "the dropped columns are a gap")
        assertTrue(history.has(20))
        assertEquals(20L, history.indexAt(1.02))
    }

    /**
     * The header's numbers, worked by hand. Leq 60, 60, 70, 70 dB: powers 1e6, 1e6, 1e7, 1e7,
     * mean 5.5e6, 10·log10(5.5e6) = 67.40 dB, not the 65 dB a mean of decibels gives.
     */
    @Test
    fun theLevelIsTheEnergyMean() {
        val history = history()
        history.record(listOf(60f, 60f, 70f, 70f).mapIndexed { index, leq -> column(index * .05, leq = leq) })
        close(67.404f, history.summary(0.0, 1.0)!!.leqDb, .001f, "LAeq")
        close((10 * log10(5.5e6)).toFloat(), energyMean(floatArrayOf(60f, 60f, 70f, 70f)), 1e-4f, "energy mean")
    }

    /**
     * L10 is exceeded 10 % of the time: the 90 % quantile of the FAST levels, as the offline
     * report takes it. LAF 30, 31, … 40 dB (11 values): positions 0.1·10 = 1 and 0.9·10 = 9 fall
     * on values, so L90 = 31 and L10 = 39 exactly.
     */
    @Test
    fun l10AndL90AreTheLevelsExceeded() {
        val history = history()
        history.record(List(11) { column(it * .05, laf = 30f + it) })
        val summary = history.summary(0.0, 1.0)!!
        close(39f, summary.l10Db, what = "L10")
        close(31f, summary.l90Db, what = "L90")
    }

    /**
     * Between values the quantile is interpolated in power, as the analyser does. LAF 40, 50,
     * 60, 70, 80: L90 at position 0.4, 0.6·1e4 + 0.4·1e5 = 4.6e4 → 46.63 dB; L10 at position 3.6,
     * 0.4·1e7 + 0.6·1e8 = 6.4e7 → 78.06 dB.
     */
    @Test
    fun betweenValuesTheQuantileIsInterpolatedInPower() {
        val levels = floatArrayOf(70f, 40f, 80f, 50f, 60f)
        close(46.628f, exceeded(levels, 90.0), .002f, "L90")
        close(78.062f, exceeded(levels, 10.0), .002f, "L10")
    }

    /**
     * Width is the mean of the columns that have one; onsets are counted a minute. 20 columns
     * (1 s) with 3 onsets → 180 a minute; widths 0.2 and 0.6 with the rest none → 0.4.
     */
    @Test
    fun widthIsAMeanAndOnsetsACountAMinute() {
        val history = history()
        history.record(
            List(20) { index ->
                column(index * .05, onset = index in setOf(2, 9, 15), width = mapOf(4 to .2f, 12 to .6f)[index])
            },
        )
        val summary = history.summary(0.0, 2.0)!!
        close(.4f, summary.width!!, 1e-6f, "width")
        close(180f, summary.onsetsPerMinute, .01f, "onsets a minute")
        assertEquals("LAeq 50 dB SPL  ·  L90 50  ·  L10 50  ·  width 0.40  ·  onsets 180/min", summaryLine(summary))
    }

    /** The span shown counts only the columns inside it: here the last half second of two. */
    @Test
    fun theSummaryCoversTheSpanShown() {
        val history = history()
        history.record(List(40) { column(it * .05, leq = if (it < 30) 40f else 70f, onset = it == 5) })
        val summary = history.summary(1.5, 2.0)
        assertNotNull(summary)
        close(70f, summary.leqDb, .001f, "only the loud columns are in the span")
        close(0f, summary.onsetsPerMinute, what = "the onset is before the span")
    }

    /** The spectrum's values read as dB SPL on the engine's scale, and its colours run as the offline picture's. */
    @Test
    fun theSpectrumReadsOnTheEnginesScale() {
        val history = history()
        history.record(listOf(column(0.0, spectrum = 255), column(.05, spectrum = 0)))
        assertEquals(255, history.spectrumAt(0, 50))
        close(90f, history.spectrumDecibels(history.spectrumAt(0, 50)), what = "the top")
        close(0f, history.spectrumDecibels(history.spectrumAt(1, 50)), what = "the floor")
        assertEquals(0x000004, spectrumColour(0), "the offline picture's darkest")
        assertEquals(0xFCFFA4, spectrumColour(255), "the offline picture's brightest")
        val brightness = (0..255).map { spectrumColour(it).let { colour -> (colour shr 16) + (colour shr 8 and 0xFF) + (colour and 0xFF) } }
        assertTrue(brightness.zipWithNext().all { (lower, higher) -> higher >= lower }, "brighter is always louder")
    }

    @Test
    fun timesAreCountedBackFromNow() {
        assertEquals("−12.3 s", agoLabel(12.34))
        assertEquals("−1:02.5", agoLabel(62.5))
    }
}
