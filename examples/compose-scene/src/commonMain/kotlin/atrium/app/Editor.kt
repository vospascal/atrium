package atrium.app

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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SceneFile

/**
 * The editor for the playing scene, in the player's paper style. In a wide
 * window it is three panels: the layers and the library on the left, the map
 * of every sound in the middle, the inspector for what is selected on the
 * right. In a narrow one the same three are tabs.
 */
@Composable
internal fun Editor(scene: SceneFile, session: Session, look: Look, onBack: () -> Unit) {
    var selected by remember(session.engine) { mutableStateOf<String?>(null) }
    var panel by remember { mutableStateOf(Panel.Layers) }
    PlayerTheme(look.appearance) {
        val tint = if (look.monochrome) LocalPlayerColors.current.monochromeInk else null
        CompositionLocalProvider(LocalInkTint provides tint) {
            // How strongly each sound reaches the listener, and nothing while a voice rests between takes.
            val resting = session.telemetry?.sources.orEmpty().filter { it.take == null }.map { it.slot }.toSet()
            val strengths = plumesFor(scene, session.description, session.telemetry).mapIndexed { index, plume ->
                if (session.description?.sources?.getOrNull(index)?.slot in resting) 0f else plume.strength
            }
            val chosen = session.description?.sources?.firstOrNull { it.id == selected }
            val deleteChosen = {
                chosen?.let(session::remove)
                selected = null
            }
            BoxWithConstraints(Modifier.fillMaxSize().background(Backdrop)) {
                val wide = maxWidth >= WideFrom
                Column(Modifier.fillMaxSize()) {
                    TopBar(scene, session, onBack)
                    Rule()
                    if (wide) {
                        // The map is always there, so the left panel only switches between its two lists.
                        val list = if (panel == Panel.Map) Panel.Layers else panel
                        Row(Modifier.weight(1f)) {
                            Column(Modifier.width(SidePanel).fillMaxHeight()) {
                                PanelTabs(listOf(Panel.Layers, Panel.Library), list) { panel = it }
                                Rule()
                                Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                                    when (list) {
                                        Panel.Library -> Library(session, onAdded = { id ->
                                            selected = id
                                            panel = Panel.Layers
                                        })
                                        else -> Layers(session, strengths, selected, onSelect = { selected = it })
                                    }
                                    Spacer(Modifier.height(24.dp))
                                }
                                if (list == Panel.Layers) {
                                    Rule()
                                    LayerActions(canDelete = chosen != null, onAdd = { panel = Panel.Library }, onDelete = deleteChosen)
                                }
                            }
                            Upright()
                            EditorMap(session, strengths, selected, onSelect = { selected = it }, Modifier.weight(1f).fillMaxHeight())
                            Upright()
                            Inspector(
                                session, scene, selected,
                                Modifier.width(SidePanel).fillMaxHeight().verticalScroll(rememberScrollState()),
                            )
                        }
                    } else {
                        PanelTabs(Panel.entries, panel) { panel = it }
                        Rule()
                        when (panel) {
                            Panel.Layers -> {
                                Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                                    Layers(session, strengths, selected, onSelect = { selected = it })
                                    Spacer(Modifier.height(24.dp))
                                }
                                Rule()
                                LayerActions(canDelete = chosen != null, onAdd = { panel = Panel.Library }, onDelete = deleteChosen)
                            }
                            Panel.Library -> Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                                Library(session, onAdded = { id ->
                                    selected = id
                                    panel = Panel.Map
                                })
                                Spacer(Modifier.height(24.dp))
                            }
                            Panel.Map -> {
                                EditorMap(session, strengths, selected, onSelect = { selected = it }, Modifier.weight(1f).fillMaxWidth())
                                Rule()
                                Inspector(
                                    session, scene, selected,
                                    Modifier.fillMaxWidth().heightIn(max = CompactInspector).verticalScroll(rememberScrollState()).padding(bottom = 18.dp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TopBar(scene: SceneFile, session: Session, onBack: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 22.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        PaperText("‹ Player", 16, Muted, modifier = Modifier.clickable(role = Role.Button, onClick = onBack))
        Spacer(Modifier.width(22.dp))
        PaperText(scene.title, 22, weight = FontWeight.Bold, modifier = Modifier.weight(1f))
        PaperText("${session.description?.sources?.size ?: 0} of $VoiceCount voices", 15, Muted)
    }
}

@Composable
private fun PanelTabs(shown: List<Panel>, selected: Panel, onSelect: (Panel) -> Unit) {
    Row(Modifier.padding(start = 10.dp, top = 12.dp, bottom = 8.dp)) {
        shown.forEach { entry ->
            PaperText(
                entry.name,
                18,
                if (entry == selected) Foreground else Muted,
                if (entry == selected) FontWeight.Bold else FontWeight.Normal,
                modifier = Modifier.clickable(role = Role.Tab) { onSelect(entry) }.padding(horizontal = 12.dp, vertical = 4.dp),
            )
        }
    }
}

/** A hairline across. */
@Composable
private fun Rule() {
    Box(Modifier.fillMaxWidth().height(1.dp).background(Hairline))
}

/** A hairline between two panels. */
@Composable
private fun Upright() {
    Box(Modifier.fillMaxHeight().width(1.dp).background(Hairline))
}

/** The three parts of the editor. Map is a tab only where the window is too narrow to show it beside the lists. */
private enum class Panel { Layers, Library, Map }

/** From this width the three panels stand side by side. */
private val WideFrom = 900.dp
private val SidePanel = 300.dp

/** How much of a narrow window the inspector may take under the map. */
private val CompactInspector = 300.dp
