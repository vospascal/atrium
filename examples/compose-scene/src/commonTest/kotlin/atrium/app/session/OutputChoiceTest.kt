package atrium.app.session

import atrium.engine.ChannelMode
import atrium.engine.Output
import atrium.engine.RenderMode
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class OutputChoiceTest {
    /** Whatever Settings can choose comes back from the store as the same output. */
    @Test
    fun everyOutputIsReadBackAsItWasSaved() {
        for (renderMode in RenderMode.entries) {
            for (channelMode in ChannelMode.entries) {
                val output = Output(renderMode, channelMode)
                assertEquals(output, outputSaved(output.saved()))
            }
        }
    }

    /** Nothing saved, or a name from another version, means the scene's own output is used. */
    @Test
    fun anUnreadableChoiceIsNoChoice() {
        assertNull(outputSaved(null))
        assertNull(outputSaved(""))
        assertNull(outputSaved("HRTF"))
        assertNull(outputSaved("HRTF/SEVEN_ONE"))
        assertNull(outputSaved("HOLOGRAM/STEREO"))
    }
}
