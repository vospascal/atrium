package atrium.app

import atrium.engine.Engine
import atrium.engine.EngineException
import atrium.engine.headTrackers
import atrium.engine.listScenes
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** Calls into the Rust engine on every target, so each one links and loads it. */
class EngineBindingTest {
    @Test
    fun aMissingSceneIsATypedErrorAndStartsNoAudio() {
        val error = assertFailsWith<EngineException.Scene> { Engine("scenes/does-not-exist.yaml") }
        assertContains(error.message.orEmpty(), "does-not-exist")
    }

    @Test
    fun listingAMissingDirectoryIsATypedError() {
        assertFailsWith<EngineException.Scene> { listScenes("no/such/directory") }
    }

    /** Settings draws one chip per tracker, so a tracker listed twice would show twice. */
    @Test
    fun eachHeadTrackerOnOfferIsListedOnce() {
        val offered = headTrackers()
        assertEquals(offered.size, offered.toSet().size)
    }
}
