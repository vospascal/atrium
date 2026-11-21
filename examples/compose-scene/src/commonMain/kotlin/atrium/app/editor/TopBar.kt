package atrium.app.editor

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SceneFile
import atrium.app.ui.Foreground
import atrium.app.ui.Muted
import atrium.app.ui.PanelSide
import atrium.app.ui.PanelToggle
import atrium.app.ui.PaperText
import atrium.app.session.Session
import atrium.app.session.VoiceCount

/**
 * The document bar: back to the player, the scene and its file with a dot
 * while it has changes that are not saved, the file actions, and in a wide
 * window the toggles for the three panels.
 */
@Composable
internal fun TopBar(
    scene: SceneFile?,
    session: Session,
    workspace: Workspace?,
    onBack: () -> Unit,
    onSave: () -> Unit,
    onSaveAs: () -> Unit,
    onOpen: () -> Unit,
) {
    // In a narrow window the scene's name gives way first, cut short, and the file name and voice
    // count are left out, so the buttons always fit on one line.
    BoxWithConstraints(Modifier.fillMaxWidth()) {
    val roomy = maxWidth >= RoomyTopBar
    Row(Modifier.fillMaxWidth().padding(horizontal = 22.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        PaperText("‹ Player", 16, Muted, singleLine = true, modifier = Modifier.clickable(role = Role.Button, onClick = onBack))
        Spacer(Modifier.width(22.dp))
        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
            PaperText(
                (scene?.title ?: "No scene open") + if (session.unsaved) "  •" else "",
                22,
                weight = FontWeight.Bold,
                singleLine = true,
                modifier = Modifier.weight(1f, fill = false),
            )
            if (roomy) {
                Spacer(Modifier.width(12.dp))
                PaperText(scene?.path?.substringAfterLast('/').orEmpty(), 14, Muted, singleLine = true, modifier = Modifier.weight(1f, fill = false))
            }
        }
        Spacer(Modifier.width(12.dp))
        val note = session.saveFailure ?: if (session.unsaved) null else "Saved"
        if (note != null && session.saves > 0) {
            PaperText(note, 14, if (session.saveFailure != null) Foreground else Muted)
            Spacer(Modifier.width(14.dp))
        }
        session.history?.let { history ->
            // Undo and redo, named by the step they take back or again.
            ToolButton("Undo", enabled = history.undoable.isNotEmpty(), onClick = session::undo)
            Spacer(Modifier.width(4.dp))
            ToolButton("Redo", enabled = history.redoable.isNotEmpty(), onClick = session::redo)
            Spacer(Modifier.width(16.dp))
        }
        if (scene != null && session.engine != null) {
            // Play and pause, as in a video editor; the space bar does the same.
            ToolButton(if (session.paused) "▶  Play" else "❚❚  Pause", strong = session.paused, onClick = session::togglePause)
            Spacer(Modifier.width(16.dp))
        }
        ToolButton("Open…", onClick = onOpen)
        if (scene != null && session.engine != null) {
            Spacer(Modifier.width(8.dp))
            ToolButton("Save", strong = session.unsaved, onClick = onSave)
            Spacer(Modifier.width(8.dp))
            ToolButton("Save as…", onClick = onSaveAs)
            if (roomy) {
                Spacer(Modifier.width(20.dp))
                PaperText("${session.description?.sources?.size ?: 0} / $VoiceCount voices", 14, Muted, singleLine = true)
            }
        }
        if (workspace != null) {
            Spacer(Modifier.width(16.dp))
            PanelToggle(PanelSide.Left, workspace.leftShown) { workspace.leftShown = !workspace.leftShown }
            PanelToggle(PanelSide.Bottom, workspace.bottomShown) { workspace.bottomShown = !workspace.bottomShown }
            PanelToggle(PanelSide.Right, workspace.rightShown) { workspace.rightShown = !workspace.rightShown }
        }
    }
    }
}

/** From this width the top bar also shows the file name and the voice count. */
private val RoomyTopBar = 1180.dp
