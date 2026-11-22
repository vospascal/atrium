package atrium.app.editor

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.foundation.focusable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isMetaPressed
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SceneFile
import atrium.app.ui.Backdrop
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ink.LocalInkTint
import atrium.app.ui.LocalPlayerColors
import atrium.app.ui.Look
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.ui.PlayerTheme
import atrium.app.ui.ScrollArea
import atrium.app.session.Session
import atrium.app.ui.Splitter
import atrium.app.ink.plumesFor

/**
 * The editor for the playing scene, in the player's paper style. In a wide
 * window the map is in the middle, with the layers and the library on the
 * left, the inspector on the right and the tracks underneath. Each of those
 * three can be hidden and resized by dragging its border, and the layout is
 * kept between launches; Tab hides or shows them all. In a narrow window the
 * same parts are tabs.
 */
@Composable
internal fun Editor(open: SceneFile?, scenes: List<SceneFile>, session: Session, look: Look, workspace: Workspace, onBack: () -> Unit) {
    var opening by remember { mutableStateOf(false) }
    PlayerTheme(look.appearance) {
        Box(
            Modifier.fillMaxSize().background(Backdrop).onPreviewKeyEvent { event ->
                val command = event.isMetaPressed || event.isCtrlPressed
                if (event.type != KeyEventType.KeyDown || !command || event.key != Key.O || opening) return@onPreviewKeyEvent false
                opening = true
                true
            },
        ) {
            when {
                open == null -> Column(Modifier.fillMaxSize()) {
                    TopBar(null, session, null, onBack, onSave = {}, onSaveAs = {}, onOpen = { opening = true })
                    Rule()
                    StartScreen(scenes, session, Modifier.weight(1f).fillMaxWidth())
                }
                session.engine == null -> Column(Modifier.fillMaxSize()) {
                    TopBar(open, session, null, onBack, onSave = {}, onSaveAs = {}, onOpen = { opening = true })
                    Rule()
                    StartingNote(open, session, Modifier.weight(1f).fillMaxWidth())
                }
                else -> SceneEditor(open, session, look, workspace, onBack, onOpen = { opening = true })
            }
            if (opening) OpenDialog(scenes, session, open, onDone = { opening = false })
        }
    }
}

/** The editor with [scene] open and playing. */
@Composable
private fun SceneEditor(scene: SceneFile, session: Session, look: Look, workspace: Workspace, onBack: () -> Unit, onOpen: () -> Unit) {
    var picked by remember(session.engine) { mutableStateOf(Selection()) }
    var panel by remember { mutableStateOf(Panel.Layers) }
    var naming by remember { mutableStateOf(false) }
    // A preset dragged from the library to the map.
    val carry = remember { Carry<atrium.engine.Preset>() }
    // Save writes over the editor's own file; a hand-written scene is saved as a copy, so it asks for a name.
    val save = { if (scene.savedByEditor) session.save(scene.path, scene.title) else naming = true }
    val keys = remember { FocusRequester() }
    LaunchedEffect(Unit) { keys.requestFocus() }
    PlayerTheme(look.appearance) {
        val tint = if (look.monochrome) LocalPlayerColors.current.monochromeInk else null
        CompositionLocalProvider(LocalInkTint provides tint) {
            // How strongly each sound reaches the listener, and nothing while a voice rests between takes.
            val resting = session.telemetry?.sources.orEmpty().filter { it.take == null }.map { it.slot }.toSet()
            val strengths = plumesFor(scene, session.description, session.telemetry).mapIndexed { index, plume ->
                if (session.description?.sources?.getOrNull(index)?.slot in resting) 0f else plume.strength
            }
            val sounds = session.description?.sources.orEmpty()
            // A sound taken out, by hand or by undo, is no longer selected.
            val selection = picked.among(sounds.map { it.id })
            val onSelection: (Selection) -> Unit = { picked = it }
            val chosen = sounds.filter { it.id in selection }
            val deleteChosen = {
                session.remove(chosen)
                picked = Selection()
            }
            val duplicateChosen: () -> Unit = { session.duplicate(chosen) { ids -> picked = Selection(ids) } }
            val heardAlone = rememberUpdatedState(chosen.map { it.id }.toSet())
            BoxWithConstraints(
                Modifier.fillMaxSize().background(Backdrop)
                    .focusRequester(keys)
                    .focusable()
                    // Hold S to hear the selection alone. It is taken here, after a text field has had
                    // the key, so an S typed into a name stays a letter.
                    .onKeyEvent { event ->
                        val plain = !(event.isMetaPressed || event.isCtrlPressed)
                        if (event.type != KeyEventType.KeyDown || event.key != Key.S || !plain || heardAlone.value.isEmpty()) {
                            return@onKeyEvent false
                        }
                        session.hearAlone(heardAlone.value)
                        true
                    }
                    .onPreviewKeyEvent { event ->
                        // Letting go of S always ends it, wherever the keys go by then.
                        if (event.type == KeyEventType.KeyUp && event.key == Key.S) session.hearAlone(emptySet())
                        if (event.type != KeyEventType.KeyDown || naming) return@onPreviewKeyEvent false
                        val command = event.isMetaPressed || event.isCtrlPressed
                        when {
                            command && event.key == Key.S -> if (event.isShiftPressed) naming = true else save()
                            !command && event.key == Key.Tab -> workspace.toggleAll()
                            !command && event.key == Key.Spacebar -> session.togglePause()
                            command && event.key == Key.Z -> if (event.isShiftPressed) session.redo() else session.undo()
                            command && event.key == Key.Y -> session.redo()
                            command && event.key == Key.D -> duplicateChosen()
                            else -> return@onPreviewKeyEvent false
                        }
                        true
                    },
            ) {
                val wide = maxWidth >= WideFrom
                Column(Modifier.fillMaxSize()) {
                    TopBar(scene, session, if (wide) workspace else null, onBack, onSave = save, onSaveAs = { naming = true }, onOpen = onOpen)
                    Rule()
                    if (wide) {
                        // The map is always there, so the left panel only switches between its two lists.
                        val list = if (panel == Panel.Map) Panel.Layers else panel
                        Row(Modifier.weight(1f)) {
                            if (workspace.leftShown) {
                                Column(Modifier.width(workspace.leftWidth).fillMaxHeight()) {
                                    PanelTabs(listOf(Panel.Layers, Panel.Library, Panel.History), list) { panel = it }
                                    Rule()
                                    ScrollArea(Modifier.weight(1f)) {
                                        when (list) {
                                            Panel.Library -> Library(session, carry, onAdded = { id ->
                                                picked = Selection(setOf(id))
                                                panel = Panel.Layers
                                            })
                                            Panel.History -> HistoryPanel(session)
                                            else -> Layers(session, strengths, selection, onSelection)
                                        }
                                        Spacer(Modifier.height(24.dp))
                                    }
                                    if (list == Panel.Layers) {
                                        Rule()
                                        LayerActions(canChange = chosen.isNotEmpty(), onAdd = { panel = Panel.Library }, onDuplicate = duplicateChosen, onDelete = deleteChosen)
                                    }
                                }
                                Splitter(acrossWidth = true) { workspace.leftWidth += it }
                            }
                            Column(Modifier.weight(1f).fillMaxHeight()) {
                                EditorMap(session, strengths, selection, onSelection, carry, Modifier.weight(1f).fillMaxWidth())
                                if (workspace.bottomShown) {
                                    Splitter(acrossWidth = false) { workspace.bottomHeight -= it }
                                    BottomPanel(session, workspace, selection, onSelection, Modifier.fillMaxWidth().height(workspace.bottomHeight))
                                }
                            }
                            if (workspace.rightShown) {
                                Splitter(acrossWidth = true) { workspace.rightWidth -= it }
                                ScrollArea(Modifier.width(workspace.rightWidth).fillMaxHeight()) {
                                    Inspector(session, scene, selection, Modifier.fillMaxWidth())
                                }
                            }
                        }
                    } else {
                        PanelTabs(Panel.entries, panel) { panel = it }
                        Rule()
                        when (panel) {
                            Panel.Layers -> {
                                Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                                    Layers(session, strengths, selection, onSelection)
                                    Spacer(Modifier.height(24.dp))
                                }
                                Rule()
                                LayerActions(canChange = chosen.isNotEmpty(), onAdd = { panel = Panel.Library }, onDuplicate = duplicateChosen, onDelete = deleteChosen)
                            }
                            Panel.Library -> Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                                Library(session, carry, onAdded = { id ->
                                    picked = Selection(setOf(id))
                                    panel = Panel.Map
                                })
                                Spacer(Modifier.height(24.dp))
                            }
                            Panel.History -> Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                                HistoryPanel(session)
                            }
                            Panel.Map -> {
                                EditorMap(session, strengths, selection, onSelection, carry, Modifier.weight(1f).fillMaxWidth())
                                Rule()
                                Inspector(
                                    session, scene, selection,
                                    Modifier.fillMaxWidth().heightIn(max = CompactInspector).verticalScroll(rememberScrollState()).padding(bottom = 18.dp),
                                )
                            }
                        }
                    }
                }
                CarryGhost(carry) { preset, over ->
                    Box(Modifier.background(if (over) Foreground else Backdrop, androidx.compose.foundation.shape.RoundedCornerShape(6.dp)).padding(horizontal = 10.dp, vertical = 5.dp)) {
                        PaperText(if (over) "Add ${preset.name} here" else preset.name, 14, if (over) Backdrop else Foreground, FontWeight.SemiBold)
                    }
                }
                if (naming) {
                    SaveAsDialog(scene, session, onDone = {
                        naming = false
                        keys.requestFocus()
                    })
                }
            }
        }
    }
}

@Composable
private fun PanelTabs(shown: List<Panel>, selected: Panel, onSelect: (Panel) -> Unit) {
    // Three tabs fit the narrowest panel; past that they share the width and are cut short.
    Row(Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 12.dp, bottom = 8.dp)) {
        shown.forEach { entry ->
            PaperText(
                entry.name,
                17,
                if (entry == selected) Foreground else Muted,
                if (entry == selected) FontWeight.Bold else FontWeight.Normal,
                singleLine = true,
                modifier = Modifier.weight(1f, fill = false).clickable(role = Role.Tab) { onSelect(entry) }.padding(horizontal = 9.dp, vertical = 4.dp),
            )
        }
    }
}

/** A hairline across. */
@Composable
private fun Rule() {
    Box(Modifier.fillMaxWidth().height(1.dp).background(Hairline))
}

/** The three parts of the editor. Map is a tab only where the window is too narrow to show it beside the lists. */
private enum class Panel { Layers, Library, History, Map }

/** From this width the three panels stand side by side. */
private val WideFrom = 900.dp

/** How much of a narrow window the inspector may take under the map. */
private val CompactInspector = 300.dp
