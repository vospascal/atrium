package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import atrium.app.session.PresetDirectory
import atrium.app.session.Session
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.ui.SmallChip
import atrium.app.ui.listRow
import atrium.engine.Preset

/**
 * The presets a sound can be made from, in the folders they lie in: Creatures, Weather, Water
 * and the rest, each folded until it is opened, and opened or folded again as it was left at
 * the last launch. The search on top shows only the presets that match, with their folders
 * open, and leaves the folders as they were once it is cleared. Preview plays one just ahead
 * of the listener until it is stopped, and shows its note when it has one; Add puts it in the
 * scene there. [onTyping] tells the editor while the search has the keys, so a space typed
 * there stays a space.
 */
@Composable
internal fun Library(
    session: Session,
    workspace: Workspace,
    carry: Carry<Preset>,
    onTyping: (Boolean) -> Unit,
    onAdded: (id: String) -> Unit,
) {
    // The preset being heard. A press starts or stops it; the sound itself follows about half
    // a second later, which is why this is a switch and not something to hold down.
    var heard by remember(session.engine) { mutableStateOf<String?>(null) }
    // Leaving the library must not leave a preset playing.
    DisposableEffect(session.engine) { onDispose { session.audition(null) } }
    var query by remember { mutableStateOf("") }
    // Folders folded by hand during one search; the next keystroke opens every match again.
    var foldedWhileSearching by remember(query) { mutableStateOf(emptySet<String>()) }
    val presets = session.presets.getOrNull().orEmpty()
    val tree = remember(presets) { presetTree(presets, PresetDirectory) }
    val view = libraryView(tree, query, workspace.libraryOpen, foldedWhileSearching)

    PaperText(
        session.libraryFailure ?: "Preview plays a sound just ahead of you until you stop it; it takes a moment to start. Add puts it in the scene there, or drag it onto the map.",
        13,
        Muted,
        modifier = Modifier.padding(start = 22.dp, end = 22.dp, top = 18.dp),
    )
    session.presets.onFailure {
        PaperText("No presets found: ${it.message}", 15, Muted, modifier = Modifier.padding(22.dp))
    }
    if (tree.count > 0) {
        SearchField(query, onChange = { query = it }, onTyping = onTyping, Modifier.padding(start = 22.dp, end = 22.dp, top = 14.dp, bottom = 6.dp))
    }
    if (view.searching && view.shown.count == 0) {
        PaperText("Nothing in the library matches “${query.trim()}”.", 14, Muted, modifier = Modifier.padding(start = 22.dp, end = 22.dp, top = 14.dp))
    }

    val rows = PresetRows(heard, carry, onHear = { preset ->
        heard = if (heard == preset.path) null else preset.path
        session.audition(heard)
    }, onAdd = { preset ->
        // Adding stops the audition in the engine, so the button goes back to play.
        heard = null
        session.add(preset.path, onAdded = onAdded)
    })
    val onToggle: (String) -> Unit = { path ->
        if (view.searching) {
            foldedWhileSearching = if (path in foldedWhileSearching) foldedWhileSearching - path else foldedWhileSearching + path
        } else {
            workspace.toggleLibraryFolder(path)
        }
    }
    // Presets lying in the presets' folder itself come first, outside any folder.
    rows.Show(view.shown.presets, depth = 0)
    view.shown.folders.forEach { folder -> Folder(folder, depth = 0, view, rows, onToggle) }
}

/** A folder's heading and, while it is open, its presets and then its sub-folders. */
@Composable
private fun Folder(folder: PresetFolder, depth: Int, view: LibraryView, rows: PresetRows, onToggle: (String) -> Unit) {
    val open = folder.path in view.open
    FolderHeading(folder, depth, open, onToggle = { onToggle(folder.path) })
    if (!open) return
    rows.Show(folder.presets, depth + 1)
    folder.folders.forEach { inside -> Folder(inside, depth + 1, view, rows, onToggle) }
}

/**
 * A folder's heading: a mark that points right while it is folded and down while it is open,
 * its title (a top folder in small capitals, as the library's groups were headed, a sub-folder
 * in plain words) and how many presets it holds. A press opens or folds it.
 */
@Composable
private fun FolderHeading(folder: PresetFolder, depth: Int, open: Boolean, onToggle: () -> Unit) {
    val top = depth == 0
    Row(
        Modifier.fillMaxWidth()
            .clickable(role = Role.Button, onClickLabel = if (open) "Fold" else "Open", onClick = onToggle)
            .semantics { stateDescription = if (open) "Open" else "Folded" }
            .padding(start = RowStart + Indent * depth, end = 22.dp, top = if (top) 16.dp else 8.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FoldMark(open)
        Spacer(Modifier.width(Indent - FoldMarkSize))
        if (top) {
            PaperText(folder.title.uppercase(), 13, Muted, FontWeight.SemiBold, spacing = 2f, singleLine = true, modifier = Modifier.weight(1f))
        } else {
            PaperText(folder.title, 15, Foreground, FontWeight.SemiBold, singleLine = true, modifier = Modifier.weight(1f))
        }
        PaperText(folder.count.toString(), 13, Muted)
    }
}

/** A small triangle: pointing right while folded, down while open. */
@Composable
private fun FoldMark(open: Boolean) {
    val ink = Muted
    Canvas(Modifier.size(FoldMarkSize)) {
        val path = Path().apply {
            if (open) {
                moveTo(size.width * .1f, size.height * .25f)
                lineTo(size.width * .9f, size.height * .25f)
                lineTo(size.width * .5f, size.height * .8f)
            } else {
                moveTo(size.width * .25f, size.height * .1f)
                lineTo(size.width * .8f, size.height * .5f)
                lineTo(size.width * .25f, size.height * .9f)
            }
            close()
        }
        drawPath(path, ink)
    }
}

/**
 * The search on top of the library: what is typed filters as it is typed. Clear, or Escape,
 * empties it. While it has the keys, [onTyping] is told, and again when it is left.
 */
@Composable
private fun SearchField(query: String, onChange: (String) -> Unit, onTyping: (Boolean) -> Unit, modifier: Modifier = Modifier) {
    var focused by remember { mutableStateOf(false) }
    val typing by rememberUpdatedState(onTyping)
    // Leaving the library with the search focused must not leave the editor's keys switched off.
    DisposableEffect(Unit) { onDispose { typing(false) } }
    Row(
        modifier.fillMaxWidth().border(1.dp, if (focused) Foreground else Hairline, RoundedCornerShape(6.dp))
            .padding(start = 10.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BasicTextField(
            query,
            onValueChange = onChange,
            singleLine = true,
            textStyle = TextStyle(color = Foreground, fontSize = 14.sp),
            cursorBrush = SolidColor(Foreground),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            decorationBox = { field ->
                Box(Modifier.padding(vertical = 3.dp)) {
                    if (query.isEmpty()) PaperText("Search by name, Hz, group or note", 14, Muted, singleLine = true)
                    field()
                }
            },
            modifier = Modifier.weight(1f)
                .onFocusChanged {
                    focused = it.isFocused
                    typing(it.isFocused)
                }
                .onPreviewKeyEvent { event ->
                    if (event.type != KeyEventType.KeyDown || event.key != Key.Escape || query.isEmpty()) return@onPreviewKeyEvent false
                    onChange("")
                    true
                },
        )
        if (query.isNotEmpty()) {
            Spacer(Modifier.width(6.dp))
            SmallChip("Clear") { onChange("") }
        }
    }
}

/** How the presets of a folder are shown: the one being [heard], and what Preview and Add do. */
private class PresetRows(
    val heard: String?,
    val carry: Carry<Preset>,
    val onHear: (Preset) -> Unit,
    val onAdd: (Preset) -> Unit,
) {
    /** [presets], indented [depth] steps, every other one shaded. */
    @Composable
    fun Show(presets: List<Preset>, depth: Int) {
        presets.forEachIndexed { row, preset ->
            PresetRow(
                row,
                preset,
                carry,
                indent = Indent * depth,
                heard = heard == preset.path,
                onHear = { onHear(preset) },
                onAdd = { onAdd(preset) },
            )
        }
    }
}

@Composable
private fun PresetRow(row: Int, preset: Preset, carry: Carry<Preset>, indent: Dp, heard: Boolean, onHear: () -> Unit, onAdd: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().listRow(row, selected = heard).padding(start = RowStart + indent, end = 22.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // The name can be dragged onto the map, to put the sound where it is let go.
        Column(Modifier.weight(1f).carrySource(carry, preset)) {
            PaperText(preset.name, 18)
            PaperText(
                when {
                    preset.generated -> "Generator"
                    preset.takes > 1u -> "${preset.takes} takes, with rests"
                    else -> "Recording"
                },
                13,
                Muted,
            )
            // The preset's note, for the one being heard: what it is and where it comes from.
            val about = preset.about
            if (heard && about != null) {
                PaperText(about, 13, Muted, modifier = Modifier.padding(top = 4.dp))
            }
        }
        // Preview plays it just ahead of you without adding it; Add puts it in the scene.
        ToolButton(if (heard) "Stop" else "Preview", strong = heard, onClick = onHear)
        Spacer(Modifier.width(6.dp))
        ToolButton("Add", onClick = onAdd)
    }
}

/** Where a row of the library starts, and how far each folder level steps in from it. */
private val RowStart = 14.dp
private val Indent = 12.dp
private val FoldMarkSize = 8.dp
