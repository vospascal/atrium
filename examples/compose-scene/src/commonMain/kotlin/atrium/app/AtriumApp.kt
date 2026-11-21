package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import atrium.engine.listScenes
import atrium.app.editor.Editor
import atrium.app.ui.Look
import atrium.app.player.Player
import atrium.app.session.RunSession
import atrium.app.session.Session
import atrium.app.editor.Workspace

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
    val scenes = remember(sceneDirectory, session.saves) { runCatching { listScenes(sceneDirectory) } }
    val look = remember { Look(settings) }
    val workspace = remember { Workspace(settings) }
    var editing by remember { mutableStateOf(false) }

    RunSession(session)

    // The editor opens with or without a scene: without one it offers the scenes to open.
    LaunchedEffect(editing) { onEditing(editing) }
    if (editing) {
        Editor(session.request?.scene, scenes.getOrNull().orEmpty(), session, look, workspace, onBack = { editing = false })
    } else {
        Player(scenes, session, look, onEdit = { editing = true })
    }
}
