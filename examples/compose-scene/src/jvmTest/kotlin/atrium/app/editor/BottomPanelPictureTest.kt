package atrium.app.editor

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.ImageComposeScene
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.unit.Density
import atrium.app.SettingsStore
import atrium.app.session.AnalysisHistory
import atrium.app.session.Session
import atrium.app.session.TrackMemory
import atrium.app.ui.Appearance
import atrium.app.ui.Backdrop
import atrium.app.ui.PlayerTheme
import atrium.engine.AnalysisColumn
import atrium.engine.AnalysisScale
import atrium.engine.Position
import atrium.engine.SourceTelemetry
import org.jetbrains.skia.EncodedImageFormat
import org.jetbrains.skia.Image
import java.io.File
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.log10
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.random.Random
import kotlin.test.Test
import kotlin.test.assertTrue

/**
 * The bottom panel drawn without a window, from made-up columns: a steady bed with a bird's
 * phrases at 2–6 kHz, the width dipping and the ear swinging while it sings. The pictures go to
 * `build/screenshots`, to be looked at; the test itself checks only that the lanes drew
 * something where they should and that ten minutes draw quickly.
 */
class BottomPanelPictureTest {
    private class Memory : SettingsStore {
        val values = mutableMapOf<String, String>()
        override fun read(key: String) = values[key]
        override fun write(key: String, value: String) {
            values[key] = value
        }
    }

    private val engineScale = AnalysisScale(0f, 90f, List(97) { 20f * 1000f.pow(it / 96f) }, .05f)

    /** Where the pictures are written: the test runs from the repository root. */
    private val screenshots = File("examples/compose-scene/build/screenshots").apply { mkdirs() }

    @Test
    fun tracksTab() {
        val (session, workspace) = sessionWithHistory(seconds = 100.0)
        workspace.bottomTab = BottomTab.Tracks
        val image = render(1100, 360) { BottomPanel(session, workspace, Selection(), {}, Modifier.fillMaxSize(), now = 100.0) }
        save(image, "bottom-tracks.png")
    }

    @Test
    fun analysisTab() {
        val (session, workspace) = sessionWithHistory(seconds = 100.0)
        workspace.bottomTab = BottomTab.Analysis
        val width = 1100
        val height = 360
        ImageComposeScene(width * 2, height * 2, Density(2f)) {
            Panel { BottomPanel(session, workspace, Selection(), {}, Modifier.fillMaxSize(), now = 100.0) }
        }.closing { scene ->
            save(scene.render(), "bottom-analysis.png")
            // Over the spectrum during the last phrase (88–90.4 s), on a chirp near 3.7 kHz, to show the readout.
            val strip = 150 * 2f..(width - 14) * 2f
            val span = 120.0 + 30.0
            val x = strip.start + ((89.0 - (100.0 - 120.0)) / span).toFloat() * (strip.endInclusive - strip.start)
            scene.sendPointerEvent(PointerEventType.Enter, Offset(x, 296f))
            scene.sendPointerEvent(PointerEventType.Move, Offset(x, 296f))
            val hovered = scene.render(16_000_000)
            save(hovered, "bottom-analysis-hover.png")
            assertSpectrumDrawn(hovered, strip)
        }
    }

    @Test
    fun analysisTabDark() {
        val (session, workspace) = sessionWithHistory(seconds = 100.0)
        workspace.bottomTab = BottomTab.Analysis
        val image = render(1100, 360, Appearance.Dark) { BottomPanel(session, workspace, Selection(), {}, Modifier.fillMaxSize(), now = 100.0) }
        save(image, "bottom-analysis-dark.png")
    }

    /**
     * Ten minutes, 12 000 columns, at the widest zoom: after the first frame builds the picture,
     * a frame takes well under a display frame, even drawn here on the processor rather than
     * the graphics card.
     */
    @Test
    fun tenMinutesDrawQuickly() {
        val history = AnalysisHistory().apply { restart(engineScale) }
        history.record(columns(0.0, TrackMemory))
        var now by mutableStateOf(TrackMemory)
        val ahead = 60.0
        ImageComposeScene(1100 * 2, 300 * 2, Density(2f)) {
            Panel { AnalysisLanes(history, now, TrackMemory, ahead, Modifier.fillMaxSize()) }
        }.closing { scene ->
            save(scene.render(), "bottom-analysis-10min.png")
            val frames = mutableListOf<Long>()
            repeat(240) { frame ->
                // The scene plays on: a frame's worth of time and, every third frame, a new column.
                now += 1.0 / 60
                if (frame % 3 == 0) history.record(columns(now - .05, now))
                val started = System.nanoTime()
                scene.render((frame + 1) * 16_666_667L)
                // The first half warms the JIT up, as a running app has long since done.
                if (frame >= 120) frames += System.nanoTime() - started
            }
            frames.sort()
            val median = frames[frames.size / 2] / 1e6
            println("ten minutes: median frame ${"%.2f".format(median)} ms, worst ${"%.2f".format(frames.last() / 1e6)} ms")
            assertTrue(median < 16.0, "a frame at ten minutes takes $median ms")
        }
    }

    /** A session with no engine, its analysis and tracks filled for the [seconds] before now. */
    private fun sessionWithHistory(seconds: Double): Pair<Session, Workspace> {
        val store = Memory()
        val session = Session(store)
        session.analysis.restart(engineScale)
        session.analysis.record(columns(0.0, seconds - .15))
        // Two voices: a bird that sings in phrases, and a stream that runs throughout.
        var now = 0.0
        while (now < seconds) {
            val bird = if (phraseAt(now) != null) SourceTelemetry(2u, Position(3f, 4f, 1f), 0f, false, Position(1f, 0f, 0f), null, (now / 12).toInt().mod(3).toUByte()) else null
            val stream = SourceTelemetry(0u, Position(-6f, 2f, 0f), 0f, false, Position(1f, 0f, 0f), null, 0u)
            session.tracks.record(now, listOfNotNull(stream, bird))
            now += 1.0 / 30
        }
        return session to Workspace(store)
    }

    /** When the bird sings: a phrase of 1.5 to 2.4 s every 12 s, starting at 4 s; how far into it [seconds] is. */
    private fun phraseAt(seconds: Double): Double? {
        val into = (seconds - 4.0).mod(12.0)
        val length = 1.5 + .3 * ((seconds - 4.0) / 12).toInt().mod(4)
        return into.takeIf { seconds >= 4.0 && it < length }
    }

    /** Made-up columns from [from] to [until] seconds: what a meadow with a bird might measure. */
    private fun columns(from: Double, until: Double): List<AnalysisColumn> {
        val random = Random((from * 1000).roundToInt())
        val rows = engineScale.rowEdgesHertz.size - 1
        fun rowOf(hertz: Double) = (rows * log10(hertz / 20.0) / 3.0).roundToInt()
        val count = ((until - from) / .05).roundToInt()
        return List(count) { index ->
            val seconds = from + index * .05
            val phrase = phraseAt(seconds)
            // Each phrase is chirps of 0.2 s with 0.1 s between, each sweeping up from 2.5 to 5.5 kHz.
            val chirp = phrase?.let { (it % .3).takeIf { into -> into < .2 } }
            val spectrum = ByteArray(rows) { row ->
                val hertz = 20.0 * 1000.0.pow((row + .5) / rows)
                // A wind bed: 38 dB SPL at the bottom, falling 4 dB an octave above 200 Hz.
                var level = 38.0 - 4.0 * maxOf(0.0, kotlin.math.log2(hertz / 200.0)) + random.nextDouble(-2.0, 2.0)
                if (chirp != null) {
                    val centre = 2500.0 * (5500.0 / 2500.0).pow(chirp / .2)
                    val away = abs(row - rowOf(centre))
                    if (away <= 4) level = maxOf(level, 66.0 - away * 4.0)
                    // The harmonic, fainter.
                    if (abs(row - rowOf(centre * 2)) <= 2) level = maxOf(level, 48.0)
                }
                (level / 90.0 * 255).roundToInt().coerceIn(0, 255).toByte()
            }
            val singing = chirp != null
            val laf = (if (singing) 61.0 else 41.0 + sin(seconds / 9.0) * 1.5) + random.nextDouble(-.8, .8)
            val leq = laf - random.nextDouble(0.0, 1.2)
            val width = if (phrase != null) .32 + random.nextDouble(-.05, .05) else .82 + random.nextDouble(-.04, .04)
            val ild = (if (phrase != null) 6.5 else 0.0) + 3.0 * sin(2 * PI * seconds / 25.0) + random.nextDouble(-.4, .4)
            AnalysisColumn(
                sceneSeconds = seconds,
                spectrum = spectrum,
                leqDb = listOf(leq.toFloat(), leq.toFloat()),
                lafDb = listOf(laf.toFloat(), laf.toFloat()),
                leqOverallDb = leq.toFloat(),
                lafOverallDb = laf.toFloat(),
                onset = chirp != null && chirp < .05,
                width = width.toFloat(),
                ildDb = ild.toFloat(),
            )
        }
    }

    @Composable
    private fun Panel(appearance: Appearance = Appearance.Light, content: @Composable () -> Unit) {
        PlayerTheme(appearance) {
            Box(Modifier.fillMaxSize().background(Backdrop)) { content() }
        }
    }

    private fun render(width: Int, height: Int, appearance: Appearance = Appearance.Light, content: @Composable () -> Unit): Image =
        ImageComposeScene(width * 2, height * 2, Density(2f)) { Panel(appearance, content) }.closing { it.render() }

    private fun save(image: Image, name: String) {
        val file = File(screenshots, name)
        file.writeBytes(image.encodeToData(EncodedImageFormat.PNG)!!.bytes)
        println("wrote ${file.absolutePath}")
    }

    /** The spectrum strip holds the colour map's bright end somewhere left of now: the bird drew. */
    private fun assertSpectrumDrawn(image: Image, strip: ClosedFloatingPointRange<Float>) {
        val bitmap = org.jetbrains.skia.Bitmap.makeFromImage(image)
        var bright = 0
        for (x in strip.start.toInt() until strip.endInclusive.toInt() step 2) {
            for (y in 0 until image.height step 2) {
                val colour = bitmap.getColor(x, y)
                val red = colour shr 16 and 0xFF
                val green = colour shr 8 and 0xFF
                val blue = colour and 0xFF
                // Inferno's orange to yellow: bright red, some green, little blue. Paper and ink are grey.
                if (red > 200 && green in 80..230 && blue < 90) bright++
            }
        }
        assertTrue(bright > 50, "the bird's chirps show in the spectrum ($bright bright pixels)")
    }

    /** Run [block] on the scene, then close it, as `use` would for a Closeable. */
    private fun <T> ImageComposeScene.closing(block: (ImageComposeScene) -> T): T = try {
        block(this)
    } finally {
        close()
    }
}
