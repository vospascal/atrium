package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import atrium.engine.listScenes

/**
 * The app: a player for the scenes in [sceneDirectory], and an editor for
 * the one playing. Both drive the same in-process engine. [onEditing] tells
 * the host when the editor opens and closes, so a desktop window can make
 * room for its three panels.
 */
@Composable
internal fun AtriumApp(sceneDirectory: String, onEditing: (Boolean) -> Unit = {}) {
    val settings = remember { platformSettings() }
    val session = remember { Session(settings) }
    val scenes = remember(sceneDirectory) { runCatching { listScenes(sceneDirectory) } }
    val look = remember { Look(settings) }
    var editing by remember { mutableStateOf(false) }

    RunSession(session)

    // The editor exists only while a scene plays.
    val playing = session.request?.scene.takeIf { editing && session.engine != null }
    LaunchedEffect(playing != null) { onEditing(playing != null) }
    if (playing != null) {
        Editor(playing, session, look, onBack = { editing = false })
    } else {
        Player(scenes, session, look, onEdit = { editing = true })
    }
}
