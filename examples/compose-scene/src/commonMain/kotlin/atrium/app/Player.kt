package atrium.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SceneFile

private enum class Tab { Home, Scenes, Settings }

/** Choose a scene and listen: Home (now playing), Scenes and Settings over one engine session. */
@Composable
internal fun Player(scenes: Result<List<SceneFile>>, session: Session, look: Look, onEdit: () -> Unit) {
    val list = scenes.getOrNull().orEmpty()
    var tab by remember { mutableStateOf(Tab.Home) }
    var view by remember { mutableStateOf(PlayerView.Zen) }
    var chosen by remember { mutableStateOf<SceneFile?>(null) }
    val current = session.request?.scene ?: chosen ?: list.firstOrNull()

    fun toggle(scene: SceneFile) {
        if (session.isActive(scene)) {
            session.stop()
        } else {
            chosen = scene
            session.play(scene)
        }
    }

    fun step(by: Int) {
        val from = current ?: return
        if (list.isEmpty()) return
        val index = list.indexOfFirst { it.path == from.path }
        val next = list[(index + by).mod(list.size)]
        chosen = next
        if (session.request != null) session.play(next)
    }

    val darkCanvas = tab == Tab.Home && current != null && view.inkStyle?.darkBackdrop == true
    PlayerTheme(look.appearance, darkCanvas) {
        val tint = if (look.monochrome) LocalPlayerColors.current.monochromeInk else null
        CompositionLocalProvider(LocalInkTint provides tint) {
            Box(Modifier.fillMaxSize().background(Backdrop)) {
                if (tab == Tab.Home && current != null) {
                    NowPlaying(
                        current, session, view, onViewChange = { view = it },
                        onToggle = { toggle(current) }, onStep = ::step,
                    ) {
                        PlayerTabs(tab, onSelect = { tab = it })
                    }
                } else {
                    Column(Modifier.fillMaxSize()) {
                        Box(Modifier.weight(1f)) {
                            when (tab) {
                                Tab.Home -> Box(Modifier.fillMaxSize().padding(24.dp)) { Wordmark() }
                                Tab.Scenes -> Scenes(scenes, session, onToggle = ::toggle)
                                Tab.Settings -> Settings(session, look, onEdit = onEdit)
                            }
                        }
                        if (current != null) {
                            MiniPlayer(current, session, onToggle = { toggle(current) }, onOpen = { tab = Tab.Home })
                        }
                        PlayerTabs(tab, onSelect = { tab = it })
                    }
                }
            }
        }
    }
}

@Composable
private fun PlayerTabs(selected: Tab, onSelect: (Tab) -> Unit) {
    Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp).height(1.dp).background(Hairline))
    Row(Modifier.fillMaxWidth().padding(vertical = 16.dp), horizontalArrangement = Arrangement.SpaceEvenly) {
        Tab.entries.forEach { entry ->
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

/** "Water · Wind · Birds". */
internal fun SceneFile.soundLine(): String = sounds.map { it.name }.distinct().joinToString(" · ")

/** What the session is doing with [scene], in a word or two. */
internal fun Session.stateOf(scene: SceneFile): String = when {
    !isActive(scene) -> "Paused"
    failure != null -> "Did not start"
    engine == null -> "Starting…"
    else -> "Playing"
}

@Composable
private fun MiniPlayer(scene: SceneFile, session: Session, onToggle: () -> Unit, onOpen: () -> Unit) {
    Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp).height(1.dp).background(Hairline))
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onOpen).padding(horizontal = 20.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        InkStill(plumesFor(scene, null, null), Modifier.size(52.dp).clip(RoundedCornerShape(4.dp)).background(Backdrop))
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            PaperText(scene.title, 17, weight = FontWeight.Bold)
            PaperText(session.stateOf(scene), 14, Muted)
        }
        RoundButton(size = 48, onClick = onToggle) {
            if (session.isActive(scene)) PauseIcon(Backdrop, 20.dp) else PlayIcon(Backdrop, 20.dp)
        }
    }
}

@Composable
internal fun RoundButton(size: Int, onClick: () -> Unit, content: @Composable () -> Unit) {
    Box(
        Modifier.size(size.dp).clip(CircleShape).background(Foreground).clickable(role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { content() }
}
