package atrium.app

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.Preset

/**
 * The presets a sound can be made from, in their groups. Holding one plays it
 * just ahead of the listener; Add puts it in the scene there.
 */
@Composable
internal fun Library(session: Session, onAdded: (id: String) -> Unit) {
    var heard by remember { mutableStateOf<String?>(null) }
    // Leaving the library must not leave a preset playing.
    DisposableEffect(session.engine) { onDispose { session.audition(null) } }

    PaperText(
        session.libraryFailure ?: "Hold a sound to hear it. Add puts it just ahead of you.",
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
        presets.forEach { preset ->
            PresetRow(
                preset,
                heard = heard == preset.path,
                onHold = { held ->
                    heard = if (held) preset.path else null
                    session.audition(if (held) preset.path else null)
                },
                onAdd = { session.add(preset.path, onAdded) },
            )
        }
    }
}

@Composable
private fun PresetRow(preset: Preset, heard: Boolean, onHold: (held: Boolean) -> Unit, onAdd: () -> Unit) {
    Row(
        Modifier.fillMaxWidth()
            .background(if (heard) Highlight else Backdrop)
            .pointerInput(preset.path) {
                detectTapGestures(onPress = {
                    onHold(true)
                    tryAwaitRelease()
                    onHold(false)
                })
            }
            .padding(horizontal = 22.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            PaperText(preset.name, 18)
            PaperText(if (preset.generated) "Generator" else "Recording", 13, Muted)
        }
        Chip("Add", selected = false, onClick = onAdd)
    }
}
