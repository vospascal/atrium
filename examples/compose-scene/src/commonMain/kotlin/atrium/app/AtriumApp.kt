package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import atrium.engine.listScenes

/**
 * The app: a player for the scenes in [sceneDirectory], and an editor for
 * the one playing. Both drive the same in-process engine.
 */
@Composable
internal fun AtriumApp(sceneDirectory: String) {
    val session = remember { Session() }
    val scenes = remember(sceneDirectory) { runCatching { listScenes(sceneDirectory) } }
    val look = remember { Look(platformSettings()) }
    var screen by remember { mutableStateOf<Screen>(Screen.Player) }

    RunSession(session)

    val playing = session.request?.scene.takeIf { session.engine != null }
    when (val shown = screen) {
        is Screen.Map if playing != null ->
            Composer(session, shown.slot, onBack = { screen = Screen.Editor })
        Screen.Editor if playing != null ->
            Editor(playing, session, look, onBack = { screen = Screen.Player }, onShowOnMap = { screen = Screen.Map(it) })
        else -> Player(scenes, session, look, onEdit = { screen = Screen.Editor })
    }
}

/** What fills the window. The editor and its map exist only while a scene plays. */
private sealed interface Screen {
    data object Player : Screen

    data object Editor : Screen

    /** The composer's map, showing the sound in [slot]. */
    data class Map(val slot: Int) : Screen
}
