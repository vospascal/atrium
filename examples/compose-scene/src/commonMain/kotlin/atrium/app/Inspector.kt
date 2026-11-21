package atrium.app

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.EmitterKind
import atrium.engine.HeadTrackingStatus
import atrium.engine.Pattern
import atrium.engine.SceneFile
import atrium.engine.SourceDescription
import kotlin.math.roundToInt

/**
 * What is selected, in the same order for every sound: what it is, how it
 * fills the air, where it is, when it plays, whether it travels. With nothing
 * selected it shows the scene itself.
 */
@Composable
internal fun Inspector(session: Session, scene: SceneFile, selected: String?, modifier: Modifier = Modifier) {
    val sounds = session.description?.sources.orEmpty()
    val sound = sounds.firstOrNull { it.id == selected }
    Column(modifier.padding(horizontal = 22.dp)) {
        if (sound != null) SoundInspector(session, sound) else SceneInspector(session, scene, sounds.size)
    }
}

@Composable
private fun SoundInspector(session: Session, sound: SourceDescription) {
    val listener = session.description?.listener ?: return
    val live = session.telemetry?.sources?.firstOrNull { it.slot == sound.slot }
    val at = live?.position ?: sound.position
    val offset = Point(at.x - listener.x, at.y - listener.y)

    PaperText(sound.name, 26, weight = FontWeight.Bold, modifier = Modifier.padding(top = 22.dp))
    PaperText(sound.group ?: UngroupedLayers, 14, Muted)

    Part("Sound")
    PaperText("Level ${sound.spl.roundToInt()} dB at 1 m", 15)
    PaperLevel(
        ((sound.spl - QuietestLayerLevel) / (LoudestLayerLevel - QuietestLayerLevel)).coerceIn(0f, 1f),
        onChange = { session.level(sound.slot, QuietestLayerLevel + it * (LoudestLayerLevel - QuietestLayerLevel)) },
        Modifier.fillMaxWidth(),
    )

    Part("Shape")
    if (sound.emitterKind != EmitterKind.FIELD) {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Pattern.entries.forEach { pattern ->
                Chip(pattern.label, selected = pattern == sound.pattern) { session.shape(sound.slot, pattern) }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
    PaperText(shapeLine(sound), 15)
    if (sound.beamHalfAngle != null) {
        PaperText("Facing ${bearingLabel(live?.facing ?: sound.facing)}. Turn it with the handle on the map.", 14, Muted)
    }

    Part("Place")
    PaperText(awayLabel(offset), 15)
    PaperText("${oneDecimal(at.z)} m above the ground", 14, Muted)

    Part("When")
    WhenPart(session, sound, live)

    Part("Moves")
    // The engine reports where a sound is, not why: one that is away from where it was placed is travelling.
    val away = Point(at.x - sound.position.x, at.y - sound.position.y)
    PaperText(if (away.groundDistanceTo(Point(0f, 0f)) > StillWithin) "Moving by itself" else "Still", 15)

    Spacer(Modifier.height(22.dp))
}

@Composable
private fun SceneInspector(session: Session, scene: SceneFile, voices: Int) {
    val description = session.description
    val telemetry = session.telemetry
    PaperText(scene.title, 26, weight = FontWeight.Bold, modifier = Modifier.padding(top = 22.dp))
    PaperText("Nothing selected. Pick a sound on the map or in the layers.", 14, Muted)

    Part("World")
    if (description != null) PaperText("${description.width.roundToInt()} × ${description.depth.roundToInt()} m", 15)
    PaperText("$voices of $VoiceCount voices", 14, Muted)

    Part("Output")
    PaperText(telemetry?.output?.let { "${it.renderMode.label} · ${it.channelMode.label}" } ?: "—", 15)
    PaperText("Volume ${((telemetry?.volume ?: 0f) * 100).roundToInt()} %", 14, Muted)

    Part("Head tracking")
    PaperText(
        when (telemetry?.headTracking) {
            HeadTrackingStatus.RECEIVING -> "Receiving · facing ${telemetry.listenerFacing.let(::bearingLabel)}"
            HeadTrackingStatus.WAITING -> "Waiting for head movement"
            else -> "Off"
        },
        15,
    )
}

@Composable
private fun Part(title: String) {
    Spacer(Modifier.height(24.dp))
    PaperText(title.uppercase(), 13, Muted, FontWeight.SemiBold, spacing = 2f)
    Spacer(Modifier.height(8.dp))
}

private val Pattern.label
    get() = when (this) {
        Pattern.OMNI -> "Omni"
        Pattern.CARDIOID -> "Focused"
        Pattern.SUPERCARDIOID -> "Narrow"
    }

/** How a sound fills the air, in a line. */
internal fun shapeLine(sound: SourceDescription): String {
    val beam = sound.beamHalfAngle
    return when {
        sound.emitterKind == EmitterKind.FIELD -> "Ambient: heard everywhere"
        beam != null -> "Focused: a beam ${(beam * 2f * 180f / kotlin.math.PI.toFloat()).roundToInt()}° wide"
        else -> "Omni: equal all round"
    }
}

/** How many sounds the engine can play at once. */
internal const val VoiceCount = 16

/** A sound closer than this to where it was placed counts as standing still (m). */
private const val StillWithin = .05f
