package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import atrium.engine.SourceDescription

/**
 * What a sound is called and how a list shows it: its name and its group, each written in
 * place, and its colour picked from swatches. Nothing here changes what is heard. A name or
 * group is kept when Return is pressed or the field is left; an empty name goes back to the
 * sound's id.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun IdentityPart(session: Session, sound: SourceDescription) {
    val keep = { name: String, color: String?, group: String? ->
        if (name != sound.name || color != sound.color || group != sound.group) session.identify(sound, name, color, group)
    }
    InPlaceField(sound.name, 26, FontWeight.Bold, Modifier.padding(top = 22.dp)) { keep(it, sound.color, sound.group) }
    InPlaceField(sound.group.orEmpty(), 14, FontWeight.Normal, placeholder = UngroupedLayers) {
        keep(sound.name, sound.color, it.ifBlank { null })
    }
    Spacer(Modifier.height(10.dp))
    val ring = Foreground
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        LayerColors.forEach { color ->
            val chosen = color.equals(sound.color, ignoreCase = true)
            Canvas(Modifier.size(20.dp).clickable(role = Role.RadioButton) { keep(sound.name, color, sound.group) }) {
                drawCircle(parseColor(color), size.minDimension / 2f - if (chosen) 3.dp.toPx() else 1.dp.toPx())
                if (chosen) drawCircle(ring, size.minDimension / 2f - .8.dp.toPx(), style = Stroke(1.4.dp.toPx()))
            }
        }
    }
}

/** Text shown as text until it is clicked, then edited where it stands; [onKeep] gets it when Return is pressed or it is left. */
@Composable
private fun InPlaceField(
    value: String,
    size: Int,
    weight: FontWeight,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    onKeep: (String) -> Unit,
) {
    var text by remember(value) { mutableStateOf(value) }
    var focused by remember { mutableStateOf(false) }
    val color = if (text.isEmpty()) Muted else Foreground
    BasicTextField(
        text,
        onValueChange = { text = it },
        singleLine = true,
        textStyle = TextStyle(color = color, fontSize = size.sp, fontWeight = weight),
        cursorBrush = SolidColor(Foreground),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onKeep(text.trim()) }),
        decorationBox = { field ->
            if (text.isEmpty() && !focused) PaperText(placeholder, size, Muted, weight) else field()
        },
        modifier = modifier.fillMaxWidth().onFocusChanged {
            if (focused && !it.isFocused) onKeep(text.trim())
            focused = it.isFocused
        },
    )
}

/** The colours a layer can be given: the ones the engine hands out by voice. */
internal val LayerColors = listOf(
    "#ff6b35", "#ffc107", "#ce93d8", "#4fc3f7", "#66bb6a", "#ef5350", "#ff8a65", "#ab47bc", "#26c6da", "#9ccc65",
)
