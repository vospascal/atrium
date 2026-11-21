package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.Preset

/**
 * The presets a sound can be made from, in their groups. The button in front
 * of one plays it just ahead of the listener until it is pressed again; Add
 * puts it in the scene there.
 */
@Composable
internal fun Library(session: Session, onAdded: (id: String) -> Unit) {
    // The preset being heard. A press starts or stops it; the sound itself follows about half
    // a second later, which is why this is a switch and not something to hold down.
    var heard by remember(session.engine) { mutableStateOf<String?>(null) }
    // Leaving the library must not leave a preset playing.
    DisposableEffect(session.engine) { onDispose { session.audition(null) } }

    PaperText(
        session.libraryFailure ?: "Press play to hear a sound just ahead of you. Add puts it in the scene there.",
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
                onHear = {
                    heard = if (heard == preset.path) null else preset.path
                    session.audition(heard)
                },
                onAdd = {
                    // Adding stops the audition in the engine, so the button goes back to play.
                    heard = null
                    session.add(preset.path, onAdded)
                },
            )
        }
    }
}

@Composable
private fun PresetRow(preset: Preset, heard: Boolean, onHear: () -> Unit, onAdd: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().background(if (heard) Highlight else Backdrop).padding(start = 14.dp, end = 22.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        HearButton(heard, onHear)
        Spacer(Modifier.width(8.dp))
        Column(Modifier.weight(1f)) {
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
        Chip("Add", selected = false, onClick = onAdd)
    }
}

/** Play while nothing is heard from this preset, stop while it is. */
@Composable
private fun HearButton(heard: Boolean, onClick: () -> Unit) {
    val color = Foreground
    Canvas(Modifier.size(34.dp).clickable(role = Role.Switch, onClick = onClick).padding(9.dp)) {
        if (heard) {
            drawRoundRect(color, Offset(size.width * .14f, size.height * .14f), Size(size.width * .72f, size.height * .72f), CornerRadius(1.5.dp.toPx()))
        } else {
            val triangle = androidx.compose.ui.graphics.Path().apply {
                moveTo(size.width * .2f, size.height * .08f)
                lineTo(size.width * .92f, size.height * .5f)
                lineTo(size.width * .2f, size.height * .92f)
                close()
            }
            drawPath(triangle, color)
        }
    }
}
