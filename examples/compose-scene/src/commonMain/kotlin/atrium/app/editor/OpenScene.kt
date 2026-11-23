package atrium.app.editor

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.layout.FlowRow
import atrium.engine.SceneFile
import atrium.engine.SceneKind
import atrium.app.ui.Chip
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.ui.ScrollArea
import atrium.app.session.Session
import atrium.app.ui.Sheet
import atrium.app.ui.listRow

/**
 * Every scene, one per row, to open one: its title, its file, how many sounds
 * it has, and whether the editor saved it. The one open now is marked.
 */
@Composable
internal fun SceneList(scenes: List<SceneFile>, open: SceneFile?, onOpen: (SceneFile) -> Unit, modifier: Modifier = Modifier) {
    ScrollArea(modifier) {
        scenes.forEachIndexed { row, scene ->
            val current = scene.path == open?.path
            Row(
                Modifier.fillMaxWidth()
                    .listRow(row, selected = current)
                    .clickable(role = Role.Button) { onOpen(scene) }
                    .padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    PaperText(scene.title, 17, weight = if (current) FontWeight.Bold else FontWeight.Normal)
                    PaperText(
                        scene.path.substringAfterLast('/') +
                            (if (scene.kind == SceneKind.LISTENING) " · listening session" else "") +
                            if (scene.savedByEditor) " · saved by the editor" else "",
                        12,
                        Muted,
                    )
                }
                PaperText(soundCount(scene.sounds.size), 13, Muted)
            }
        }
    }
}

private fun soundCount(count: Int) = if (count == 1) "1 sound" else "$count sounds"

/**
 * What opening [scene] would throw away: nothing, so it opens at once, or the
 * unsaved changes of the scene open now, which are put to the person first.
 */
internal fun mustAskBeforeOpening(scene: SceneFile, open: SceneFile?, unsaved: Boolean): Boolean =
    unsaved && open != null && scene.path != open.path

/**
 * Open another scene from the editor, or start a new one. The scene open now
 * stops and the chosen one starts; the editor stays. With unsaved changes it
 * asks first.
 */
@Composable
internal fun OpenDialog(scenes: List<SceneFile>, session: Session, open: SceneFile?, directory: String, onDone: () -> Unit) {
    // What would open, by its title, and how to open it, while the person is asked about their changes.
    var asking by remember { mutableStateOf<Pair<String, () -> Unit>?>(null) }
    val choose = { scene: SceneFile ->
        when {
            scene.path == open?.path -> onDone()
            mustAskBeforeOpening(scene, open, session.unsaved) -> asking = scene.title to { session.play(scene) }
            else -> {
                session.play(scene)
                onDone()
            }
        }
    }
    val startNew = { kind: SceneKind ->
        val start = { session.playNew(kind, directory) }
        if (session.unsaved && open != null) {
            asking = "a new ${kind.label.lowercase()}" to start
        } else {
            start()
            onDone()
        }
    }
    Sheet(width = 520.dp) {
        val waiting = asking
        if (waiting == null) {
            PaperText("Open a scene", 22, weight = FontWeight.Bold)
            Spacer(Modifier.height(12.dp))
            SceneList(scenes, open, choose, Modifier.fillMaxWidth().heightIn(max = 360.dp))
            Spacer(Modifier.height(16.dp))
            NewScenePart(startNew)
            Spacer(Modifier.height(16.dp))
            Row(Modifier.fillMaxWidth()) {
                Spacer(Modifier.weight(1f))
                ToolButton("Cancel", onClick = onDone)
            }
        } else {
            PaperText("Discard your changes?", 22, weight = FontWeight.Bold)
            Spacer(Modifier.height(8.dp))
            PaperText("${open?.title ?: "This scene"} has changes that are not saved. Opening ${waiting.first} throws them away.", 15)
            Spacer(Modifier.height(20.dp))
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Spacer(Modifier.weight(1f))
                ToolButton("Cancel", onClick = { asking = null })
                ToolButton("Discard and open", strong = true) {
                    waiting.second()
                    onDone()
                }
            }
        }
    }
}

/**
 * The editor with no scene open, like a picture editor with no document: the
 * scenes to open one from, and a new one to start. Opening one plays it.
 */
@Composable
internal fun StartScreen(scenes: List<SceneFile>, session: Session, directory: String, modifier: Modifier = Modifier) {
    Box(modifier, contentAlignment = Alignment.Center) {
        Column(Modifier.widthIn(max = 560.dp).fillMaxSize().padding(vertical = 40.dp)) {
            PaperText("Open a scene", 28, weight = FontWeight.Bold)
            PaperText("It starts playing, and you can change it while you listen.", 15, Muted)
            Spacer(Modifier.height(16.dp))
            SceneList(scenes, null, onOpen = session::play, Modifier.fillMaxWidth().weight(1f))
            Spacer(Modifier.height(16.dp))
            NewScenePart { kind -> session.playNew(kind, directory) }
        }
    }
}

/**
 * A new, empty scene: only its kind is asked. A nature scene starts in the meadow, a listening
 * session in the dry space; the place is chosen in the scene, in the World panel. It opens unsaved.
 */
@Composable
internal fun NewScenePart(onNew: (kind: SceneKind) -> Unit) {
    var kind by remember { mutableStateOf(SceneKind.NATURE) }
    PaperText("NEW SCENE", 12, Muted, FontWeight.SemiBold, spacing = 2f)
    Spacer(Modifier.height(8.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SceneKind.entries.forEach { choice -> Chip(choice.label, selected = choice == kind) { kind = choice } }
    }
    Spacer(Modifier.height(8.dp))
    PaperText(newSceneNote(kind), 13, Muted)
    Spacer(Modifier.height(10.dp))
    ToolButton("New ${kind.label.lowercase()}", strong = true) { onNew(kind) }
}

/** Where a new scene of [kind] starts, and where its place is changed. */
internal fun newSceneNote(kind: SceneKind): String = when (kind) {
    SceneKind.NATURE -> "Starts in the meadow, on a calm day. Choose another place in the World panel."
    SceneKind.LISTENING -> "Starts dry: no room, direct sound only, and no weather, daylight or air. Choose a place in the World panel."
}

/** While a scene is starting, or why it did not. */
@Composable
internal fun StartingNote(scene: SceneFile, session: Session, modifier: Modifier = Modifier) {
    Box(modifier, contentAlignment = Alignment.Center) {
        val failure = session.failure
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            PaperText(if (failure == null) "Starting ${scene.title}…" else "${scene.title} did not start", 18, weight = FontWeight.Bold)
            if (failure != null) {
                Spacer(Modifier.height(6.dp))
                PaperText(failure, 14, Muted, modifier = Modifier.width(420.dp))
            }
        }
    }
}
