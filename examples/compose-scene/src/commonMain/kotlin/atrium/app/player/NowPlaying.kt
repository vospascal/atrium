package atrium.app.player

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SceneFile
import atrium.app.ui.Backdrop
import atrium.app.ink.InkField
import atrium.app.ink.InkStyle
import atrium.app.ui.LocalPlayerColors
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.session.Session
import atrium.app.ui.Wordmark
import atrium.app.ink.plumesFor

internal enum class PlayerView(val label: String, val inkStyle: InkStyle? = null) {
    Zen2("Zen 2", InkStyle.Layered),
    Zen3("Zen 3", InkStyle.Smoke),
    Map("3D Map"),
}

/** The scene in full: its ink or its map, its name and sounds, and transport. */
@Composable
internal fun NowPlaying(
    scene: SceneFile,
    session: Session,
    view: PlayerView,
    onViewChange: (PlayerView) -> Unit,
    onToggle: () -> Unit,
    onStep: (Int) -> Unit,
    navigation: @Composable () -> Unit,
) {
    val active = session.isActive(scene)
    val description = if (active) session.description else null
    val telemetry = if (active) session.telemetry else null
    // Pausing cuts off the sources while the pigment already in the water can fade.
    val playing = active && session.engine != null
    val plumes = plumesFor(scene, description, telemetry)

    Box(Modifier.fillMaxSize()) {
        val style = view.inkStyle
        if (style != null) {
            InkField(
                if (playing) plumes else plumes.map { it.copy(strength = 0f) },
                LocalPlayerColors.current.ring,
                flowKey = scene.path,
                modifier = Modifier.matchParentSize(),
                style = style,
            )
        }
        Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f).fillMaxWidth()) {
                if (view == PlayerView.Map) {
                    SceneMap(
                        scene, description, telemetry, plumes,
                        Modifier.fillMaxSize().padding(top = 120.dp, bottom = 260.dp),
                    )
                }
                Column(Modifier.fillMaxSize().padding(horizontal = 24.dp)) {
                    Row(Modifier.fillMaxWidth().padding(top = 22.dp), verticalAlignment = Alignment.CenterVertically) {
                        Wordmark()
                        Spacer(Modifier.weight(1f))
                        if (active) PaperText("NOW PLAYING", 13, Muted, FontWeight.SemiBold, spacing = 2f)
                    }
                    Spacer(Modifier.height(28.dp))
                    ViewToggle(view, onViewChange, Modifier.align(Alignment.CenterHorizontally))
                    Spacer(Modifier.weight(1f))
                    PaperText(scene.title, 40, weight = FontWeight.Bold)
                    Spacer(Modifier.height(4.dp))
                    PaperText(scene.soundLine(), 17)
                    val failure = session.failure
                    if (active && failure != null) {
                        Spacer(Modifier.height(8.dp))
                        PaperText("Did not start: $failure", 13, Color(0xFFD0583F))
                    }
                    Spacer(Modifier.height(32.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                        PaperText("Prev", 18, weight = FontWeight.SemiBold, modifier = Modifier.clickable(role = Role.Button) { onStep(-1) }.padding(12.dp))
                        Spacer(Modifier.width(40.dp))
                        RoundButton(size = 96, onClick = onToggle) {
                            PaperText(if (active) "Pause" else "Play", 20, Backdrop, FontWeight.SemiBold)
                        }
                        Spacer(Modifier.width(40.dp))
                        PaperText("Next", 18, weight = FontWeight.SemiBold, modifier = Modifier.clickable(role = Role.Button) { onStep(1) }.padding(12.dp))
                    }
                    Spacer(Modifier.height(28.dp))
                }
            }
            navigation()
        }
    }
}

/** Switch visualization without changing playback. */
@Composable
private fun ViewToggle(view: PlayerView, onChange: (PlayerView) -> Unit, modifier: Modifier) {
    val colors = LocalPlayerColors.current
    val shape = RoundedCornerShape(50)
    BoxWithConstraints(modifier.width(480.dp)) {
        val labelSize = if (maxWidth < 350.dp) 13 else 17
        Row(Modifier.fillMaxWidth().clip(shape).background(colors.other).border(1.5.dp, colors.foreground, shape)) {
            PlayerView.entries.forEach { entry ->
                val chosen = entry == view
                Box(
                    Modifier.weight(1f)
                        .clip(shape)
                        .background(if (chosen) colors.chosen else colors.other)
                        .selectable(selected = chosen, role = Role.Tab) { onChange(entry) }
                        .padding(vertical = 12.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    PaperText(entry.label, labelSize, if (chosen) colors.onChosen else colors.onOther, FontWeight.SemiBold)
                }
            }
        }
    }
}
