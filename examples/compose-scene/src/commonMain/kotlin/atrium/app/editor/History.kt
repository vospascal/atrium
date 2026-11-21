package atrium.app.editor

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.EditHistory
import atrium.app.ui.Foreground
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.session.Session
import atrium.app.ui.listRow

/** Where [history] stands as one list: the opened scene, each step taken, then each that can be redone. */
internal data class HistoryLine(val labels: List<String>, val current: Int)

internal fun historyLine(history: EditHistory): HistoryLine =
    HistoryLine(listOf("Opened") + history.undoable + history.redoable, history.undoable.size)

/**
 * The History panel, as in Photoshop: every step taken since the scene opened, the one the scene
 * is at marked, and steps undone still there, faint, until a new step replaces them. Choosing one
 * takes the scene back or forward to it.
 */
@Composable
internal fun HistoryPanel(session: Session) {
    val history = session.history ?: return
    val line = historyLine(history)
    Column(Modifier.fillMaxWidth().padding(top = 4.dp)) {
        line.labels.forEachIndexed { index, label ->
            val undone = index > line.current
            Row(
                Modifier.fillMaxWidth()
                    .listRow(index, selected = index == line.current)
                    .clickable(role = Role.Button) { session.travelTo(index) }
                    .padding(horizontal = 18.dp, vertical = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                PaperText(
                    label,
                    16,
                    if (undone) Muted else Foreground,
                    if (index == line.current) FontWeight.Bold else FontWeight.Normal,
                )
            }
        }
    }
}
