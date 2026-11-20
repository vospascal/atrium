package atrium.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SceneFile

/**
 * The editor for the playing scene, in the player's paper style: its sounds as
 * layers, and the library a new one comes from. A layer's place is edited on
 * the composer's map.
 */
@Composable
internal fun Editor(scene: SceneFile, session: Session, look: Look, onBack: () -> Unit, onShowOnMap: (slot: Int) -> Unit) {
    var selected by remember(session.engine) { mutableStateOf<String?>(null) }
    var panel by remember { mutableStateOf(Panel.Layers) }
    PlayerTheme(look.appearance) {
        val tint = if (look.monochrome) LocalPlayerColors.current.monochromeInk else null
        CompositionLocalProvider(LocalInkTint provides tint) {
            Column(Modifier.fillMaxSize().background(Backdrop).verticalScroll(rememberScrollState())) {
                PaperText(
                    "‹ Player",
                    16,
                    Muted,
                    modifier = Modifier.padding(start = 22.dp, top = 22.dp).clickable(role = Role.Button, onClick = onBack),
                )
                PaperText(scene.title, 40, weight = FontWeight.Bold, modifier = Modifier.padding(start = 22.dp, top = 18.dp))
                val sounds = session.description?.sources.orEmpty()
                PaperText(
                    "${sounds.size} of $VoiceCount voices",
                    15,
                    Muted,
                    modifier = Modifier.padding(start = 22.dp, top = 4.dp),
                )
                Row(Modifier.padding(start = 10.dp, top = 18.dp)) {
                    Panel.entries.forEach { entry ->
                        PaperText(
                            entry.name,
                            18,
                            if (entry == panel) Foreground else Muted,
                            if (entry == panel) FontWeight.Bold else FontWeight.Normal,
                            modifier = Modifier.clickable(role = Role.Tab) { panel = entry }.padding(horizontal = 12.dp, vertical = 4.dp),
                        )
                    }
                }
                Box(Modifier.fillMaxWidth().padding(horizontal = 22.dp).height(1.dp).background(Hairline))
                when (panel) {
                    Panel.Layers -> Layers(
                        session,
                        strengths = plumesFor(scene, session.description, session.telemetry).map { it.strength },
                        selected = selected,
                        onSelect = { selected = it },
                        onShowOnMap = onShowOnMap,
                    )
                    // A sound that was just added is shown as a layer, selected.
                    Panel.Library -> Library(session, onAdded = { id ->
                        selected = id
                        panel = Panel.Layers
                    })
                }
                Spacer(Modifier.height(28.dp))
            }
        }
    }
}

/** How many sounds the engine can play at once. */
private const val VoiceCount = 16

/** The two lists of the editor. */
private enum class Panel { Layers, Library }
