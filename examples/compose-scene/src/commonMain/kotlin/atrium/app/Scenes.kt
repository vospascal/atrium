package atrium.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SceneFile

/** Every scene, each with its own ink as a cover. */
@Composable
internal fun Scenes(scenes: Result<List<SceneFile>>, session: Session, onToggle: (SceneFile) -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        Box(Modifier.padding(start = 24.dp, top = 22.dp)) { Wordmark() }
        PaperText("Scenes", 52, weight = FontWeight.Bold, modifier = Modifier.padding(start = 22.dp, top = 24.dp, bottom = 20.dp))
        scenes.onFailure {
            PaperText("No scenes found: ${it.message}", 15, Muted, modifier = Modifier.padding(24.dp))
        }
        scenes.getOrNull().orEmpty().forEach { scene ->
            SceneRow(scene, session, onClick = { onToggle(scene) })
        }
    }
}

@Composable
private fun SceneRow(scene: SceneFile, session: Session, onClick: () -> Unit) {
    val active = session.isActive(scene)
    val cover = remember(scene) { plumesFor(scene, null, null) }
    Row(
        Modifier.fillMaxWidth()
            .background(if (active) Highlight else Backdrop)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 22.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        InkStill(cover, Modifier.size(88.dp).clip(RoundedCornerShape(4.dp)).background(Backdrop))
        Spacer(Modifier.width(18.dp))
        Column(Modifier.weight(1f)) {
            PaperText(scene.title, 22, weight = FontWeight.Bold)
            Spacer(Modifier.height(4.dp))
            PaperText(scene.soundLine(), 15, Muted)
        }
        if (active) LevelBarsIcon(playing = session.engine != null, size = 26.dp) else PlayIcon(Foreground, 26.dp, outline = true)
    }
    Box(Modifier.fillMaxWidth().padding(horizontal = 22.dp).height(1.dp).background(Hairline))
}
