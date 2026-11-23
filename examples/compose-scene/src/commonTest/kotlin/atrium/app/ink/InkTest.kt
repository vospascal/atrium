package atrium.app.ink

import atrium.engine.SceneKind
import atrium.engine.ChannelMode
import atrium.engine.Bout
import atrium.engine.EmitterKind
import atrium.engine.HeadTrackingStatus
import atrium.engine.Output
import atrium.engine.Position
import atrium.engine.RenderMode
import atrium.engine.SceneDescription
import atrium.engine.SceneFile
import atrium.engine.SoundSummary
import atrium.engine.SourceDescription
import atrium.engine.SourceSignal
import atrium.engine.SourceTelemetry
import atrium.engine.Take
import atrium.engine.Telemetry
import kotlin.math.PI
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue
import atrium.app.Point

class InkTest {
    private val listener = Position(30f, 30f, 1.2f)

    private fun scene(vararg offsets: Position) = SceneFile(
        name = "test",
        path = "scenes/test.yaml",
        title = "Test",
        kind = SceneKind.NATURE,
        savedByEditor = false,
        sounds = offsets.mapIndexed { index, offset -> SoundSummary("Sound $index", "#26c6da", offset) },
    )

    private fun description(
        count: Int,
        spl: Float = 65f,
        kind: EmitterKind = EmitterKind.OBJECT,
        beam: Float? = null,
    ) = SceneDescription(
        kind = SceneKind.NATURE,
        space = "environments/meadow.yaml",
        width = 60f,
        depth = 60f,
        temperatureC = 20f,
        humidityPct = 50f,
        listener = listener,
        sources = List(count) { SourceDescription("sound-$it", it.toUShort(), 360f, -60f, "Sound $it", null, "#26c6da", listener, spl, 1f, kind, beam, 0f, 360f, Position(1f, 0f, 0f), listOf(Take("Take", 1f)), null, true, Bout(1u, 1u), emptyList(), emptyList()) },
        speakers = emptyList(),
    )

    private fun heard(at: Position, gainDb: Float, muted: Boolean = false, signal: SourceSignal? = null) = Telemetry(
        sources = listOf(SourceTelemetry(0u, at, gainDb, muted, Position(1f, 0f, 0f), signal, 0u)),
        peaksDb = listOf(-20f, -20f),
        output = Output(RenderMode.HRTF, ChannelMode.STEREO),
        channelsInUse = emptyList(),
        volume = 1f,
        listenerFacing = Position(1f, 0f, 0f),
        headTracking = HeadTrackingStatus.OFF,
    )

    private fun close(expected: Float, actual: Float) =
        assertTrue(kotlin.math.abs(expected - actual) < 1e-4f, "expected $expected, got $actual")

    /**
     * A sound added from the library is in the engine's description and not in
     * the scene file. It gets a plume like the others, in its own color.
     */
    @Test
    fun aSoundAddedWhilePlayingGetsAPlume() {
        val file = scene(Position(10f, 0f, 0f))
        assertEquals(1, plumesFor(file, null, null).size)
        val plumes = plumesFor(file, description(2), null)
        assertEquals(2, plumes.size)
        assertEquals(parseColor("#26c6da"), plumes[1].color)
    }

    /**
     * After a sound is removed the scene's list has a gap in its voices. Each plume
     * still follows the sound with its own voice, not the one at the same place in the list.
     */
    @Test
    fun aPlumeFollowsItsOwnVoiceAfterAnotherWasRemoved() {
        val three = description(3)
        val left = three.copy(sources = three.sources.filter { it.slot != 1.toUShort() })
        val west = Position(listener.x - 8f, listener.y, 0f)
        val live = heard(west, gainDb = 0f).let { it.copy(sources = it.sources.map { source -> source.copy(slot = 2u) }) }
        val plumes = plumesFor(scene(Position(10f, 0f, 0f)), left, live)
        assertEquals(2, plumes.size)
        assertEquals(0f, plumes[0].strength, "voice 0 is not in the telemetry")
        assertTrue(plumes[1].strength > 0f, "the second sound left is the one in voice 2")
        close(PI.toFloat(), kotlin.math.abs(plumes[1].direction))
    }

    @Test
    fun inkFlowsTowardTheSoundBeforeItPlays() {
        val plumes = plumesFor(scene(Position(10f, 0f, 0f), Position(0f, 10f, 0f)), null, null)
        close(0f, plumes[0].direction)
        close((-PI / 2).toFloat(), plumes[1].direction)
        assertTrue(plumes.all { it.strength == RestingStrength })
    }

    @Test
    fun whilePlayingInkFollowsWhereTheSoundIsNow() {
        val west = Position(listener.x - 8f, listener.y, 0f)
        val plume = plumesFor(scene(Position(10f, 0f, 0f)), description(1), heard(west, gainDb = -10f)).single()
        close(PI.toFloat(), kotlin.math.abs(plume.direction))
    }

    @Test
    fun louderSoundsAreDenserInk() {
        val at = Position(listener.x + 10f, listener.y, 0f)
        val sounds = scene(Position(10f, 0f, 0f))
        val loud = plumesFor(sounds, description(1), heard(at, gainDb = 0f)).single()
        val quiet = plumesFor(sounds, description(1), heard(at, gainDb = -30f)).single()
        val inaudible = plumesFor(sounds, description(1), heard(at, gainDb = -120f)).single()
        assertEquals(1f, loud.strength)
        assertTrue(quiet.strength < loud.strength && quiet.strength > inaudible.strength)
        assertTrue(inaudible.strength > 0f, "an inaudible sound stays a faint trace, not a hole")
    }

    @Test
    fun audioLevelAndFrequencyDriveEachZenMode() {
        val sounds = scene(Position(10f, 0f, 0f))
        fun plume(level: Float, lowShare: Float, highShare: Float, gainDb: Float = 0f, muted: Boolean = false) = plumesFor(
            sounds, description(1, spl = 50f),
            heard(listener, gainDb, muted, SourceSignal(level, lowShare, highShare, listOf(level))),
        ).single()

        val low = plume(1f, .95f, .02f)
        val high = plume(1f, .02f, .95f)
        val mids = plume(1f, 0f, 0f)
        val quiet = plume(.2f, .95f, .02f)
        val distant = plume(1f, .95f, .02f, gainDb = -15f)
        val zen = InkLayer.Cover.band
        close(2f / 3f, low.forBand(zen).strength)
        close(low.forBand(zen).strength, high.forBand(zen).strength)

        for (style in InkStyle.entries.filter { it.layers.size == 2 }) {
            val (wash, detail) = style.layers.map { it.band }
            assertTrue(low.forBand(wash).strength > low.forBand(detail).strength, "$style bass")
            assertTrue(high.forBand(detail).strength > high.forBand(wash).strength, "$style treble")
            close(mids.forBand(wash).strength, mids.forBand(detail).strength)
        }
        for (style in InkStyle.entries) {
            for (layer in style.layers) {
                val band = layer.band
                assertTrue(quiet.forBand(band).strength < low.forBand(band).strength, "$layer level")
                assertTrue(distant.forBand(band).strength < low.forBand(band).strength, "$layer attenuation")
                assertEquals(0f, plume(0f, 0f, 0f).forBand(band).strength, "$layer silence")
                assertEquals(0f, plume(1f, .95f, .02f, muted = true).forBand(band).strength, "$layer mute")
            }
        }
    }

    @Test
    fun aMutedSoundLeavesNoInk() {
        val at = Position(listener.x + 10f, listener.y, 0f)
        val plume = plumesFor(scene(Position(10f, 0f, 0f)), description(1), heard(at, gainDb = 0f, muted = true)).single()
        assertEquals(0f, plume.strength)
    }

    @Test
    fun fieldsUseTheirBearingWhenSetAndSurroundTheListenerOtherwise() {
        val built = description(1, kind = EmitterKind.FIELD)
        val offset = scene(Position(1f, 0f, 0f))
        val west = listener.copy(x = listener.x - 8f)
        val fan = plumesFor(offset, built, heard(west, gainDb = 0f)).single()
        assertEquals(PlumeExtent.Fan, fan.extent)
        close(PI.toFloat(), kotlin.math.abs(fan.direction))
        assertEquals(PlumeExtent.Surround, plumesFor(offset, built, heard(listener, 0f)).single().extent)
    }

    @Test
    fun localizedPlumesKeepTheBuiltBeamWidth() {
        val sounds = scene(Position(2f, 0f, 0f))
        assertNull(plumesFor(sounds, description(1), null).single().beamHalfAngle)
        val beam = plumesFor(sounds, description(1, beam = .32f), null).single()
        assertEquals(PlumeExtent.Point, beam.extent)
        assertEquals(.32f, beam.beamHalfAngle)
    }

    @Test
    fun aSourceMissingFromLiveTelemetryStopsReleasingInk() {
        val live = heard(listener, gainDb = 0f)
        val twoSounds = scene(Position(2f, 0f, 0f), Position(-2f, 0f, 0f))
        val plumes = plumesFor(twoSounds, description(2), live)
        assertTrue(plumes.first().strength > 0f)
        assertEquals(0f, plumes.last().strength)
    }

    @Test
    fun sceneColorsAreParsedAndJunkIsGrey() {
        assertEquals(0xFF26C6DA.toInt(), parseColor("#26c6da").toArgbInt())
        assertEquals(parseColor("not a color"), parseColor("#zz"))
    }

    private fun androidx.compose.ui.graphics.Color.toArgbInt(): Int =
        ((alpha * 255).toInt() shl 24) or ((red * 255).toInt() shl 16) or ((green * 255).toInt() shl 8) or (blue * 255).toInt()
}
