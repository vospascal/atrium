package atrium.app.session

import atrium.engine.Position
import atrium.engine.SourceSignal
import atrium.engine.SourceTelemetry
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import atrium.app.editor.channelNames

class TrackHistoryTest {
    private fun voice(slot: Int, take: Int?, muted: Boolean = false) =
        SourceTelemetry(slot.toUShort(), Position(0f, 0f, 0f), 0f, muted, Position(1f, 0f, 0f), null, take?.toUByte())

    /** A voice that utters, rests and utters again leaves two clips with a gap; the take is kept. */
    @Test
    fun takesAndRestsBecomeClipsAndGaps() {
        val history = TrackHistory()
        var now = 0.0
        repeat(30) { history.record(now, listOf(voice(3, 1))); now += 1.0 / 60 }
        repeat(60) { history.record(now, listOf(voice(3, null))); now += 1.0 / 60 }
        repeat(30) { history.record(now, listOf(voice(3, 2))); now += 1.0 / 60 }
        val clips = history.clipsOf(3)
        assertEquals(listOf(1, 2), clips.map { it.take })
        assertTrue(clips[1].start - clips[0].end > .9, "the rest is a gap")
        assertTrue(clips[0].levels.size in 4..6, "a level every tenth of a second")
        assertTrue(history.clipsOf(0).isEmpty(), "an empty voice has no clips")
    }

    /** A take that changes straight into another, or a mute, starts a new clip; old clips fall off the window. */
    @Test
    fun aNewTakeOrAMuteStartsANewClipAndOldOnesFallOff() {
        val history = TrackHistory(window = 10.0)
        history.record(0.0, listOf(voice(0, 0)))
        history.record(.1, listOf(voice(0, 1)))
        history.record(.2, listOf(voice(0, 1, muted = true)))
        assertEquals(listOf(0, 1, 1), history.clipsOf(0).map { it.take })
        assertTrue(history.clipsOf(0).last().muted)
        history.record(30.0, listOf(voice(0, 1)))
        assertEquals(1, history.clipsOf(0).size, "only what is inside the window is kept")
    }

    /** A sound with layers keeps each layer's level beside the voice's, on the same scale; a muted layer is flat. */
    @Test
    fun eachLayerKeepsItsOwnLevel() {
        val history = TrackHistory()
        var now = 0.0
        val signal = SourceSignal(1f, .3f, .3f, listOf(1f, .5f, 0f))
        repeat(30) {
            history.record(now, listOf(SourceTelemetry(2u, Position(0f, 0f, 0f), 0f, false, Position(1f, 0f, 0f), signal, 0u)))
            now += 1.0 / 60
        }
        val clip = history.clipsOf(2).single()
        assertEquals(clip.levels.size, clip.layers.size, "a level of each layer at every step")
        val (whole, half, silent) = clip.layers.first()
        assertEquals(1f, whole)
        assertEquals((36f - 6.0206f) / 36f, half, 1e-3f)
        assertEquals(0f, silent)
        val oneStream = SourceSignal(1f, .3f, .3f, listOf(1f))
        history.record(now + 1, listOf(SourceTelemetry(4u, Position(0f, 0f, 0f), 0f, false, Position(1f, 0f, 0f), oneStream, 0u)))
        assertEquals(listOf(emptyList()), history.clipsOf(4).single().layers, "one stream has no layers")
    }
}

class ChannelNamesTest {
    @kotlin.test.Test
    fun channelsAreNamedAsTheRoomNamesThem() {
        val room = mapOf(0 to "L", 1 to "R", 2 to "C", 4 to "Ls", 5 to "Rs")
        kotlin.test.assertEquals(listOf("L", "R", "C", "LFE", "Ls", "Rs"), channelNames(6, headphones = false, room))
        kotlin.test.assertEquals(listOf("L", "R"), channelNames(2, headphones = true, room))
        kotlin.test.assertEquals(listOf("1", "2", "3", "4"), channelNames(4, headphones = false, emptyMap()))
    }
}
