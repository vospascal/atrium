package atrium.app

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.Preset

/**
 * The presets a sound can be made from, in their groups. Preview plays one
 * just ahead of the listener until it is stopped; Add puts it in the scene
 * there.
 */
@Composable
internal fun Library(session: Session, carry: Carry<Preset>, onAdded: (id: String) -> Unit) {
    // The preset being heard. A press starts or stops it; the sound itself follows about half
    // a second later, which is why this is a switch and not something to hold down.
    var heard by remember(session.engine) { mutableStateOf<String?>(null) }
    // Leaving the library must not leave a preset playing.
    DisposableEffect(session.engine) { onDispose { session.audition(null) } }

    PaperText(
        session.libraryFailure ?: "Preview plays a sound just ahead of you until you stop it; it takes a moment to start. Add puts it in the scene there, or drag it onto the map.",
        13,
        Muted,
        modifier = Modifier.padding(start = 22.dp, end = 22.dp, top = 18.dp),
    )
    session.presets.onFailure {
        PaperText("No presets found: ${it.message}", 15, Muted, modifier = Modifier.padding(22.dp))
    }
    groupedInSceneOrder(session.presets.getOrNull().orEmpty()) { it.group }.forEach { (group, presets) ->
        PaperText(
            group.uppercase(),
            13,
            Muted,
            FontWeight.SemiBold,
            spacing = 2f,
            modifier = Modifier.padding(start = 22.dp, top = 22.dp, bottom = 6.dp),
        )
        presets.forEachIndexed { row, preset ->
            PresetRow(
                row,
                preset,
                carry,
                heard = heard == preset.path,
                onHear = {
                    heard = if (heard == preset.path) null else preset.path
                    session.audition(heard)
                },
                onAdd = {
                    // Adding stops the audition in the engine, so the button goes back to play.
                    heard = null
                    session.add(preset.path, onAdded = onAdded)
                },
            )
        }
    }
}

@Composable
private fun PresetRow(row: Int, preset: Preset, carry: Carry<Preset>, heard: Boolean, onHear: () -> Unit, onAdd: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().listRow(row, selected = heard).padding(start = 14.dp, end = 22.dp, top = 6.dp, bottom = 6.dp),
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
        }
        // Preview plays it just ahead of you without adding it; Add puts it in the scene.
        ToolButton(if (heard) "Stop" else "Preview", strong = heard, onClick = onHear)
        Spacer(Modifier.width(6.dp))
        ToolButton("Add", onClick = onAdd)
    }
}
