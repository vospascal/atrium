package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import atrium.engine.SourceDescription
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ink.LocalInkTint
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.session.Session
import atrium.app.ui.listRow
import atrium.app.ink.parseColor

/**
 * The sounds of the playing scene as layers, in their groups. The ear in
 * front of a layer silences it and brings it back, the way the eye in a layer
 * list hides a layer; a group's ear does it for the whole group. A click
 * selects a layer, and with Shift or ⌘ held adds it to the others.
 *
 * [strengths] is how strongly each sound reaches the listener right now, from
 * 0 to 1, in the order the scene lists its sounds.
 */
@Composable
internal fun Layers(
    session: Session,
    strengths: List<Float>,
    selection: Selection,
    onSelection: (Selection) -> Unit,
) {
    val sounds = session.description?.sources.orEmpty()
    val keys = remember { HeldKeys() }
    val mix = session.mix
    var row = 0
    groupedInSceneOrder(sounds.withIndex().toList()) { it.value.group }.forEach { (group, members) ->
        val ids = members.map { it.value.id }
        Row(
            Modifier.fillMaxWidth().padding(start = 12.dp, end = 14.dp, top = 12.dp, bottom = 1.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Ear(hearing = !mix.muted.containsAll(ids), Muted, size = CompactEar) { session.mix(mix.toggleMuteAll(ids)) }
            Spacer(Modifier.width(6.dp))
            PaperText(group.uppercase(), 11, Muted, FontWeight.SemiBold, spacing = 1.5f, singleLine = true)
        }
        members.forEach { (index, sound) ->
            LayerRow(
                row++,
                sound,
                strength = if (mix.silences(sound.id)) 0f else strengths.getOrElse(index) { 0f },
                hearing = !mix.silences(sound.id),
                selected = sound.id in selection,
                onEar = { session.mix(mix.toggleMute(sound.id)) },
                onSelect = { onSelection(selection.clickedInList(sound.id, keys.adding)) },
                modifier = keys.watch,
            )
        }
    }
}

@Composable
private fun LayerRow(
    row: Int,
    sound: SourceDescription,
    strength: Float,
    hearing: Boolean,
    selected: Boolean,
    onEar: () -> Unit,
    onSelect: () -> Unit,
    modifier: Modifier,
) {
    val ink = LocalInkTint.current ?: parseColor(sound.color)
    Row(
        modifier.fillMaxWidth()
            .listRow(row, selected)
            .clickable(role = Role.Button, onClick = onSelect)
            .padding(start = 12.dp, end = 14.dp, top = 2.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Ear(hearing, Foreground, size = CompactEar, onClick = onEar)
        Spacer(Modifier.width(6.dp))
        Canvas(Modifier.size(8.dp)) { drawCircle(ink.copy(alpha = if (hearing) 1f else .35f)) }
        Spacer(Modifier.width(8.dp))
        PaperText(sound.name, 15, if (hearing) Foreground else Muted, singleLine = true, modifier = Modifier.weight(1f))
        Spacer(Modifier.width(8.dp))
        Meter(strength, Modifier.width(56.dp).height(10.dp))
    }
}

/** The ear in front of a layer and a group, small enough for a dense list but still easy to hit. */
private val CompactEar = 22.dp

/**
 * The bar under the layers, where a layer list keeps its actions: add a sound
 * from the library, and duplicate or delete the selected layer.
 */
@Composable
internal fun LayerActions(canChange: Boolean, onAdd: () -> Unit, onDuplicate: () -> Unit, onDelete: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Spacer(Modifier.weight(1f))
        ActionIcon(Foreground, onAdd) { color -> plus(color) }
        Spacer(Modifier.width(6.dp))
        ActionIcon(if (canChange) Foreground else Hairline, onClick = { if (canChange) onDuplicate() }) { color -> copies(color) }
        Spacer(Modifier.width(6.dp))
        ActionIcon(if (canChange) Foreground else Hairline, onClick = { if (canChange) onDelete() }) { color -> bin(color) }
    }
}

@Composable
private fun ActionIcon(color: Color, onClick: () -> Unit, draw: androidx.compose.ui.graphics.drawscope.DrawScope.(Color) -> Unit) {
    Canvas(Modifier.size(34.dp).clickable(role = Role.Button, onClick = onClick).padding(8.dp)) { draw(color) }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.plus(color: Color) {
    val stroke = 1.8.dp.toPx()
    drawLine(color, Offset(size.width / 2, size.height * .12f), Offset(size.width / 2, size.height * .88f), stroke, StrokeCap.Round)
    drawLine(color, Offset(size.width * .12f, size.height / 2), Offset(size.width * .88f, size.height / 2), stroke, StrokeCap.Round)
}

/** Two sheets, one over the other: a copy. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.copies(color: Color) {
    val stroke = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.6.dp.toPx())
    val corner = androidx.compose.ui.geometry.CornerRadius(2.dp.toPx())
    val sheet = androidx.compose.ui.geometry.Size(size.width * .62f, size.height * .62f)
    drawRoundRect(color, Offset(size.width * .08f, size.height * .08f), sheet, corner, style = stroke)
    drawRoundRect(color, Offset(size.width * .30f, size.height * .30f), sheet, corner, style = stroke)
}

/** A waste bin: a lid with a handle, and a tapering body with two ribs. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.bin(color: Color) {
    val stroke = 1.6.dp.toPx()
    val w = size.width
    val h = size.height
    drawLine(color, Offset(w * .08f, h * .24f), Offset(w * .92f, h * .24f), stroke, StrokeCap.Round)
    drawLine(color, Offset(w * .38f, h * .08f), Offset(w * .62f, h * .08f), stroke, StrokeCap.Round)
    val body = Path().apply {
        moveTo(w * .2f, h * .24f)
        lineTo(w * .27f, h * .95f)
        lineTo(w * .73f, h * .95f)
        lineTo(w * .8f, h * .24f)
    }
    drawPath(body, color, style = Stroke(stroke))
    drawLine(color, Offset(w * .42f, h * .42f), Offset(w * .43f, h * .78f), stroke, StrokeCap.Round)
    drawLine(color, Offset(w * .58f, h * .42f), Offset(w * .57f, h * .78f), stroke, StrokeCap.Round)
}

/**
 * An ear: drawn whole for a layer that is heard, faint and struck through for
 * one that is silenced. Press it to change that.
 */
@Composable
internal fun Ear(hearing: Boolean, color: Color, size: Dp = 26.dp, onClick: () -> Unit) {
    Canvas(Modifier.size(size).clickable(role = Role.Switch, onClick = onClick).padding(4.dp)) {
        val w = this.size.width
        val h = this.size.height
        val ink = color.copy(alpha = if (hearing) 1f else .35f)
        val stroke = Stroke(1.6.dp.toPx(), cap = StrokeCap.Round)
        // The rim: up the back, over the top, down the front into the lobe.
        val rim = Path().apply {
            moveTo(w * .40f, h * .96f)
            cubicTo(w * .14f, h * .86f, w * .30f, h * .64f, w * .24f, h * .44f)
            cubicTo(w * .16f, h * .14f, w * .50f, h * .00f, w * .72f, h * .12f)
            cubicTo(w * .96f, h * .26f, w * .90f, h * .54f, w * .72f, h * .64f)
            cubicTo(w * .58f, h * .72f, w * .68f, h * .90f, w * .40f, h * .96f)
        }
        drawPath(rim, ink, style = stroke)
        // The fold inside it.
        val fold = Path().apply {
            moveTo(w * .42f, h * .46f)
            cubicTo(w * .38f, h * .26f, w * .66f, h * .22f, w * .68f, h * .40f)
        }
        drawPath(fold, ink, style = stroke)
        if (!hearing) {
            drawLine(color.copy(alpha = .7f), Offset(w * .08f, h * .92f), Offset(w * .92f, h * .08f), 1.6.dp.toPx(), StrokeCap.Round)
        }
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

/** The sounds of a scene in their groups, each group where its first sound stands in the scene. */
internal fun <T> groupedInSceneOrder(sounds: List<T>, groupOf: (T) -> String?): List<Pair<String, List<T>>> =
    sounds.groupBy { groupOf(it) ?: UngroupedLayers }.toList()

/** The heading for sounds whose scene gives them no group. */
internal const val UngroupedLayers = "Other"

/** The quietest and loudest level a layer's slider reaches, in dB SPL at 1 m. */
internal const val QuietestLayerLevel = 20f
internal const val LoudestLayerLevel = 100f
