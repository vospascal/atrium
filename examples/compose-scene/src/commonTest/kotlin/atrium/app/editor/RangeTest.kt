package atrium.app.editor

import kotlin.test.Test
import kotlin.test.assertEquals

class RangeTest {
    /** Two handles on one spot, as a band from 0 to 0 or a bout of 1 to 1: a press beside them takes the handle on that side. */
    @Test
    fun handlesOnOneSpotAreTakenByTheSideOfThePress() {
        assertEquals(HighHandle, handleAt(.4f, 0f, 0f), "right of a 0–0 band moves its high end")
        assertEquals(LowHandle, handleAt(.2f, .5f, .5f))
        assertEquals(Undecided, handleAt(.5f, .5f, .5f), "right on them, the drag decides")
    }

    @Test
    fun otherwiseTheNearerHandleIsTaken() {
        assertEquals(LowHandle, handleAt(.1f, .2f, .8f))
        assertEquals(HighHandle, handleAt(.7f, .2f, .8f))
    }
}
