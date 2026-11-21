package atrium.app

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
import kotlin.time.TimeSource

/**
 * Which edits belong to one step of undo.
 *
 * A slider dragged or a sound moved sends many edits a second, and undo should take all of them
 * back at once, as one step. Edits of the same kind to the same thing, each soon after the one
 * before, are one step; anything else starts a new one. This is how a text editor groups typing,
 * and it needs nothing from the controls that make the edits.
 */
internal class Steps(private val clock: () -> Long = monotonicMillis()) {
    private var lastKey: String? = null
    private var lastAt = 0L

    /** Whether an edit known by [key] (such as "level:3") starts a new step now. */
    fun starts(key: String): Boolean {
        val at = clock()
        val starts = key != lastKey || at - lastAt > SameStepWithinMillis
        lastKey = key
        lastAt = at
        return starts
    }

    /** The next edit starts a new step, whatever it is: after an undo, nothing joins the step before. */
    fun end() {
        lastKey = null
    }
}

private fun monotonicMillis(): () -> Long {
    val start = TimeSource.Monotonic.markNow()
    return { start.elapsedNow().inWholeMilliseconds }
}

/** Edits further apart than this are separate steps, even of the same thing. */
internal const val SameStepWithinMillis = 1000L

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
