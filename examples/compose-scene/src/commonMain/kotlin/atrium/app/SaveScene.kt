package atrium.app

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import atrium.engine.SceneFile
import atrium.engine.listScenes

/**
 * The file a scene called [title] is saved to: its name in lower case, with a
 * dash for every run of other characters. "Meadow at dusk" becomes
 * `meadow-at-dusk`.
 */
internal fun fileNameFor(title: String): String =
    title.lowercase()
        .replace(Regex("[^a-z0-9]+"), "-")
        .trim('-')
        .ifEmpty { "untitled" }

/** What happens when a scene called [title] is saved into [scenes]. */
internal sealed interface SaveTarget {
    val path: String

    /** No file of that name yet. */
    data class New(override val path: String) : SaveTarget

    /** A file the editor wrote before: saving replaces it. */
    data class Replaces(override val path: String) : SaveTarget

    /** A hand-written file. It keeps its comments, so it is never saved over. */
    data class HandWritten(override val path: String) : SaveTarget
}

internal fun saveTarget(title: String, directory: String, scenes: List<SceneFile>): SaveTarget {
    val path = "$directory/${fileNameFor(title)}.yaml"
    val existing = scenes.firstOrNull { it.path == path } ?: return SaveTarget.New(path)
    return if (existing.savedByEditor) SaveTarget.Replaces(path) else SaveTarget.HandWritten(path)
}

/**
 * Asks for a name and saves the playing scene under it, in the folder [scene]
 * came from. It starts from the scene's own title, so a hand-written scene is
 * saved as a copy beside it.
 */
@Composable
internal fun SaveAsDialog(scene: SceneFile, session: Session, onDone: () -> Unit) {
    val directory = scene.path.substringBeforeLast('/', ".")
    val scenes = remember(directory) { runCatching { listScenes(directory) }.getOrDefault(emptyList()) }
    var title by remember { mutableStateOf(if (scene.savedByEditor) scene.title else "${scene.title} edit") }
    val target = saveTarget(title, directory, scenes)
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    val save = {
        if (target !is SaveTarget.HandWritten && title.isNotBlank()) {
            session.save(target.path, title.trim())
            if (session.saveFailure == null) onDone()
        }
    }

    Sheet {
            PaperText("Save as", 22, weight = FontWeight.Bold)
            Spacer(Modifier.height(16.dp))
            PaperText("NAME", 12, Muted, FontWeight.SemiBold, spacing = 2f)
            Spacer(Modifier.height(6.dp))
            BasicTextField(
                title,
                onValueChange = { title = it },
                singleLine = true,
                textStyle = TextStyle(color = Foreground, fontSize = 18.sp),
                cursorBrush = SolidColor(Foreground),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { save() }),
                modifier = Modifier.fillMaxWidth()
                    .focusRequester(focus)
                    .border(1.dp, Foreground, RoundedCornerShape(6.dp))
                    .padding(horizontal = 12.dp, vertical = 10.dp),
            )
            Spacer(Modifier.height(8.dp))
            PaperText(
                when (target) {
                    is SaveTarget.New -> target.path
                    is SaveTarget.Replaces -> "${target.path} · replaces the earlier save"
                    is SaveTarget.HandWritten -> "${target.path} is written by hand. Pick another name."
                },
                13,
                if (target is SaveTarget.HandWritten) Foreground else Muted,
            )
            session.saveFailure?.let {
                Spacer(Modifier.height(8.dp))
                PaperText(it, 13, Foreground)
            }
            Spacer(Modifier.height(20.dp))
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Spacer(Modifier.weight(1f))
                ToolButton("Cancel", onClick = onDone)
                Spacer(Modifier.width(8.dp))
                ToolButton("Save", strong = true, enabled = target !is SaveTarget.HandWritten && title.isNotBlank(), onClick = save)
            }
    }
}

/**
 * A button in a tool bar: a label in a thin frame. [strong] fills it, for the
 * one action a bar or a dialog is for.
 */
@Composable
internal fun ToolButton(label: String, strong: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val shape = RoundedCornerShape(6.dp)
    val ink = if (enabled) Foreground else Hairline
    Box(
        Modifier.border(1.dp, ink, shape)
            .background(if (strong && enabled) Foreground else Backdrop, shape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 6.dp),
    ) {
        PaperText(label, 14, if (strong && enabled) Backdrop else ink, FontWeight.SemiBold, singleLine = true)
    }
}
