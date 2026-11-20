package atrium.app

import atrium.engine.Engine
import atrium.engine.EngineException
import atrium.engine.listScenes
import kotlin.test.Test
import kotlin.test.assertContains
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
}
