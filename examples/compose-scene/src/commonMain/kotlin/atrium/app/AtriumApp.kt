package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import atrium.engine.listScenes

/**
 * The app: a player for the scenes in [sceneDirectory], and a composer for
 * the one playing. Both drive the same in-process engine.
 */
@Composable
internal fun AtriumApp(sceneDirectory: String) {
    val session = remember { Session() }
    val scenes = remember(sceneDirectory) { runCatching { listScenes(sceneDirectory) } }
    val look = remember { Look(platformSettings()) }
    var composing by remember { mutableStateOf(false) }

    RunSession(session)

    if (composing && session.engine != null) {
        Composer(session, onBack = { composing = false })
    } else {
        Player(scenes, session, look, onCompose = { composing = true })
    }
}
