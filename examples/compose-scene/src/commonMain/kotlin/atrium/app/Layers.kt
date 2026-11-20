package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.SourceDescription
import kotlin.math.roundToInt

/**
 * The sounds of the playing scene as layers, in their groups. Each layer can
 * be silenced, heard alone and selected; the selected one shows its level.
 *
 * [strengths] is how strongly each sound reaches the listener right now, from
 * 0 to 1 in slot order.
 */
@Composable
internal fun Layers(
    session: Session,
    strengths: List<Float>,
    selected: String?,
    onSelect: (String?) -> Unit,
    onShowOnMap: (slot: Int) -> Unit,
) {
    val sounds = session.description?.sources.orEmpty()
    val mix = session.mix
    groupedInSceneOrder(sounds.withIndex().toList()) { it.value.group }.forEach { (group, members) ->
        val ids = members.map { it.value.id }
        Row(
            Modifier.fillMaxWidth().padding(start = 22.dp, end = 22.dp, top = 22.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            AudibleMark(audible = !mix.muted.containsAll(ids), Foreground) { session.mix(mix.toggleMuteAll(ids)) }
            Spacer(Modifier.width(12.dp))
            PaperText(group.uppercase(), 13, Muted, FontWeight.SemiBold, spacing = 2f)
        }
        members.forEach { (slot, sound) ->
            LayerRow(
                sound,
                strength = if (mix.silences(sound.id)) 0f else strengths.getOrElse(slot) { 0f },
                audible = sound.id !in mix.muted,
                alone = sound.id in mix.solo,
                selected = sound.id == selected,
                onMute = { session.mix(mix.toggleMute(sound.id)) },
                onSolo = { session.mix(mix.toggleSolo(sound.id)) },
                onSelect = { onSelect(if (sound.id == selected) null else sound.id) },
            )
            if (sound.id == selected) LayerLevel(session, slot, sound, onShowOnMap = { onShowOnMap(slot) })
        }
    }
}

@Composable
private fun LayerRow(
    sound: SourceDescription,
    strength: Float,
    audible: Boolean,
    alone: Boolean,
    selected: Boolean,
    onMute: () -> Unit,
    onSolo: () -> Unit,
    onSelect: () -> Unit,
) {
    val ink = LocalInkTint.current ?: parseColor(sound.color)
    Row(
        Modifier.fillMaxWidth()
            .background(if (selected) Highlight else Backdrop)
            .clickable(role = Role.Button, onClick = onSelect)
            .padding(horizontal = 22.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AudibleMark(audible, ink, onMute)
        Spacer(Modifier.width(12.dp))
        PaperText(sound.name, 18, if (audible) Foreground else Muted, modifier = Modifier.weight(1f))
        Meter(strength, Modifier.width(72.dp).height(14.dp))
        Spacer(Modifier.width(14.dp))
        SoloMark(alone, onSolo)
    }
}

/** The selected layer's level, and the way to its place on the map. */
@Composable
private fun LayerLevel(session: Session, slot: Int, sound: SourceDescription, onShowOnMap: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Highlight).padding(start = 58.dp, end = 22.dp, bottom = 14.dp)) {
        PaperText("Level ${sound.spl.roundToInt()} dB at 1 m", 13, Muted)
        PaperLevel(
            ((sound.spl - QuietestLayerLevel) / (LoudestLayerLevel - QuietestLayerLevel)).coerceIn(0f, 1f),
            onChange = { session.level(slot, QuietestLayerLevel + it * (LoudestLayerLevel - QuietestLayerLevel)) },
            Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(6.dp))
        Chip("Show on map", selected = false, onClick = onShowOnMap)
    }
}

/** A filled dot for a layer that sounds, a ring for one that is muted. Press to change it. */
@Composable
private fun AudibleMark(audible: Boolean, color: Color, onClick: () -> Unit) {
    Canvas(Modifier.size(24.dp).clickable(role = Role.Switch, onClick = onClick)) {
        val radius = 6.dp.toPx()
        if (audible) drawCircle(color, radius) else drawCircle(color, radius, style = Stroke(1.5.dp.toPx()))
    }
}

/** "S": filled while this layer is heard alone. */
@Composable
private fun SoloMark(alone: Boolean, onClick: () -> Unit) {
    val shape = RoundedCornerShape(4.dp)
    Box(
        Modifier.size(26.dp)
            .border(1.dp, if (alone) Foreground else Hairline, shape)
            .background(if (alone) Foreground else Backdrop, shape)
            .clickable(role = Role.Switch, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        PaperText("S", 13, if (alone) Backdrop else Muted, FontWeight.SemiBold)
    }
}

/** How strongly a layer reaches the listener, 0 to 1. */
@Composable
private fun Meter(strength: Float, modifier: Modifier) {
    val track = Hairline
    val fill = Foreground
    Canvas(modifier) {
        val middle = size.height / 2
        drawLine(track, Offset(0f, middle), Offset(size.width, middle), strokeWidth = 3.dp.toPx())
        if (strength > 0f) {
            drawLine(fill, Offset(0f, middle), Offset(size.width * strength.coerceIn(0f, 1f), middle), strokeWidth = 3.dp.toPx())
        }
    }
}
