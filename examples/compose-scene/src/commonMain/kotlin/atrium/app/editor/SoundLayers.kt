package atrium.app.editor

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import atrium.app.session.Session
import atrium.app.ui.Muted
import atrium.app.ui.PaperLevel
import atrium.app.ui.PaperText
import atrium.app.ui.SmallChip
import atrium.engine.SoundLayer
import atrium.engine.SourceDescription
import kotlin.math.roundToInt

/** The softest and loudest a layer can be set against the layer as its generator makes it (dB): the engine's range. */
internal const val QuietestLayerDb = -60f
internal const val LoudestLayerDb = 12f

/** Where [levelDb] sits along a layer's fader, 0 to 1, evenly in dB. */
internal fun layerFader(levelDb: Float): Float = ((levelDb - QuietestLayerDb) / (LoudestLayerDb - QuietestLayerDb)).coerceIn(0f, 1f)

/** The level a layer's fader at [fraction] of its track sets (dB), to half a dB. */
internal fun layerLevelAt(fraction: Float): Float =
    ((QuietestLayerDb + fraction.coerceIn(0f, 1f) * (LoudestLayerDb - QuietestLayerDb)) * 2f).roundToInt() / 2f

/** A layer's level as its fader shows it: "0 dB", "−6 dB", "+3.5 dB". */
internal fun layerLevelLabel(levelDb: Float): String {
    val rounded = (levelDb * 2f).roundToInt() / 2f
    val magnitude = kotlin.math.abs(rounded)
    val number = if (magnitude == magnitude.roundToInt().toFloat()) "${magnitude.roundToInt()}" else "$magnitude"
    return when {
        rounded > 0f -> "+$number dB"
        rounded < 0f -> "−$number dB"
        else -> "0 dB"
    }
}

/**
 * A sound's layers as faders, each with its solo and mute: 0 dB is the layer as its generator makes
 * it, so a sound with every fader at 0 sounds as it did before. Nothing for a sound of one stream.
 */
@Composable
internal fun LayersPart(session: Session, sound: SourceDescription) {
    if (sound.layers.isEmpty()) return
    Spacer(Modifier.height(10.dp))
    PaperText("Layers", 15)
    sound.layers.forEach { layer -> LayerFader(session, sound, layer) }
}

@Composable
private fun LayerFader(session: Session, sound: SourceDescription, layer: SoundLayer) {
    Spacer(Modifier.height(6.dp))
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        PaperText(layer.name, 14, modifier = Modifier.weight(1f))
        PaperText(layerLevelLabel(layer.levelDb), 13, Muted)
        Spacer(Modifier.width(2.dp))
        SmallChip("S", selected = layer.solo) { session.layer(sound, layer, layer.levelDb, !layer.solo, layer.mute) }
        SmallChip("M", selected = layer.mute) { session.layer(sound, layer, layer.levelDb, layer.solo, !layer.mute) }
    }
    PaperLevel(
        layerFader(layer.levelDb),
        onChange = { session.layer(sound, layer, layerLevelAt(it), layer.solo, layer.mute) },
        // Half a dB an arrow press.
        step = .5f / (LoudestLayerDb - QuietestLayerDb),
        modifier = Modifier.fillMaxWidth(),
    )
}
