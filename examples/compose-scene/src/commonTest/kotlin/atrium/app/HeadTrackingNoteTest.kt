package atrium.app

import atrium.engine.ChannelMode
import atrium.engine.HeadTrackingStatus
import atrium.engine.Output
import atrium.engine.Position
import atrium.engine.RenderMode
import atrium.engine.Telemetry
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class HeadTrackingNoteTest {
    private fun live(status: HeadTrackingStatus, renderMode: RenderMode, facing: Position = Position(0f, 1f, 0f)) =
        Telemetry(
            sources = emptyList(),
            peaksDb = listOf(-20f, -20f),
            output = Output(renderMode, ChannelMode.STEREO),
            volume = 1f,
            listenerFacing = facing,
            headTracking = status,
        )

    /** Scene files measure bearings clockwise from north (+Y); the read-out matches them. */
    @Test
    fun aFacingIsReadAsACompassBearing() {
        assertEquals("0° N", bearingLabel(Position(0f, 1f, 0f)))
        assertEquals("90° E", bearingLabel(Position(1f, 0f, 0f)))
        assertEquals("180° S", bearingLabel(Position(0f, -1f, 0f)))
        assertEquals("270° W", bearingLabel(Position(-1f, 0f, 0f)))
        assertEquals("45° NE", bearingLabel(Position(1f, 1f, 0f)))
        assertEquals("359° N", bearingLabel(Position(-0.0175f, 1f, 0f)))
    }

    @Test
    fun theMapSaysWhereTheHeadPointsWhileATrackerSends() {
        assertEquals(
            "Head tracking · facing 90° E",
            headTrackingNote(live(HeadTrackingStatus.RECEIVING, RenderMode.HRTF, Position(1f, 0f, 0f))),
        )
        assertEquals(
            "Head tracking · waiting for head movement",
            headTrackingNote(live(HeadTrackingStatus.WAITING, RenderMode.HRTF)),
        )
    }

    /** Speakers do not follow a head, so only a headphone output is told where to switch it on. */
    @Test
    fun theHintToSwitchItOnIsForHeadphonesOnly() {
        assertEquals(
            "Head tracking is off · switch it on in Settings",
            headTrackingNote(live(HeadTrackingStatus.OFF, RenderMode.HRTF)),
        )
        assertNull(headTrackingNote(live(HeadTrackingStatus.OFF, RenderMode.VBAP)))
    }
}
