package atrium.app.editor

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
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
import atrium.engine.SceneFile
import atrium.engine.SourceDescription
import kotlin.math.roundToInt
import atrium.app.ui.Muted
import atrium.app.ui.PaperLevel
import atrium.app.ui.PaperText
import atrium.app.Point
import atrium.app.session.Session
import atrium.app.session.VoiceCount
import atrium.app.bearingLabel
import atrium.app.player.label

/**
 * What is selected, in the same order for every sound: what it is, how it
 * fills the air, where it is, when it plays, whether it travels. With nothing
 * selected it shows the scene itself, and with several what can be done to
 * them together.
 */
@Composable
internal fun Inspector(session: Session, scene: SceneFile, selection: Selection, modifier: Modifier = Modifier) {
    val sounds = session.description?.sources.orEmpty()
    val chosen = sounds.filter { it.id in selection }
    Column(modifier.padding(horizontal = 22.dp)) {
        when {
            chosen.size > 1 -> SeveralInspector(chosen)
            chosen.size == 1 -> SoundInspector(session, chosen.single())
            selection.single == ListenerId -> ListenerInspector(session)
            else -> SceneInspector(session, scene, sounds.size)
        }
    }
}

/** Several sounds selected: which, and what applies to all of them at once. */
@Composable
private fun SeveralInspector(sounds: List<SourceDescription>) {
    PaperText("${sounds.size} sounds", 26, weight = FontWeight.Bold, modifier = Modifier.padding(top = 22.dp))
    PaperText("Select one of them alone to change its sound, shape or rhythm.", 14, Muted)

    Part("Selected")
    sounds.forEach { PaperText(it.name, 15) }

    Part("Together")
    PaperText("Drag one on the map and they all move; the arrow keys nudge them.", 14, Muted)
    PaperText("Hold Alt while dragging to leave copies where you let go.", 14, Muted)
    PaperText("Hold S to hear only these. ⌘D duplicates them; the bin under the layers removes them.", 14, Muted)

    Spacer(Modifier.height(22.dp))
}

@Composable
private fun SoundInspector(session: Session, sound: SourceDescription) {
    val listener = session.description?.listener ?: return
    val live = session.telemetry?.sources?.firstOrNull { it.slot == sound.slot }
    val at = live?.position ?: sound.position
    val offset = Point(at.x - listener.x, at.y - listener.y)

    IdentityPart(session, sound)

    Part("Sound")
    PaperText(if (sound.emitterKind == EmitterKind.FIELD) "Level ${sound.spl.roundToInt()} dB, everywhere" else "Level ${sound.spl.roundToInt()} dB at 1 m", 15)
    PaperLevel(
        ((sound.spl - QuietestLayerLevel) / (LoudestLayerLevel - QuietestLayerLevel)).coerceIn(0f, 1f),
        onChange = { session.level(sound.slot, QuietestLayerLevel + it * (LoudestLayerLevel - QuietestLayerLevel)) },
        // Half a dB an arrow press.
        step = .5f / (LoudestLayerLevel - QuietestLayerLevel),
        modifier = Modifier.fillMaxWidth(),
    )

    Part("Shape")
    ShapePart(session, sound, live)

    Part("Place")
    PaperText(awayLabel(offset), 15)
    PaperText("${oneDecimal(at.z)} m above the ground", 14, Muted)

    Part("When")
    WhenPart(session, sound, live)

    Part("Follows")
    FollowsPart(sound.follows)

    Part("Moves")
    // The engine reports where a sound is, not why: one that is away from where it was placed is travelling.
    val away = Point(at.x - sound.position.x, at.y - sound.position.y)
    PaperText(if (away.groundDistanceTo(Point(0f, 0f)) > StillWithin) "Moving by itself" else "Still", 15)

    Spacer(Modifier.height(22.dp))
}

/** Where the listener stands and faces: what every sound is heard from. */
@Composable
private fun ListenerInspector(session: Session) {
    val listener = session.description?.listener ?: return
    val telemetry = session.telemetry
    PaperText("Listener", 26, weight = FontWeight.Bold, modifier = Modifier.padding(top = 22.dp))
    PaperText("Where every sound is heard from. Drag it on the map, turn it by its handle, or nudge it with the arrow keys.", 14, Muted)

    Part("Place")
    PaperText("${oneDecimal(listener.x)} m east · ${oneDecimal(listener.y)} m north", 15)
    PaperText("Ears ${oneDecimal(listener.z)} m above the ground", 14, Muted)

    Part("Facing")
    PaperText(telemetry?.listenerFacing?.let(::bearingLabel) ?: "—", 15)
    if (telemetry?.headTracking == HeadTrackingStatus.RECEIVING) {
        PaperText("Head tracking turns it from here.", 14, Muted)
    }
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

    Part("Weather")
    WeatherPart(session)

    if (description != null) {
        Part("Air")
        val temperature = description.temperatureC
        val humidity = description.humidityPct
        PaperText("Temperature ${oneDecimal(temperature)} °C", 15)
        PaperLevel(
            (temperature - ColdestAir) / (WarmestAir - ColdestAir),
            onChange = { session.atmosphere(ColdestAir + it * (WarmestAir - ColdestAir), humidity) },
            // Half a degree an arrow press.
            step = .5f / (WarmestAir - ColdestAir),
            modifier = Modifier.fillMaxWidth(),
        )
        PaperText("Humidity ${humidity.roundToInt()} %", 15)
        PaperLevel(
            humidity / 100f,
            onChange = { session.atmosphere(temperature, it * 100f) },
            modifier = Modifier.fillMaxWidth(),
        )
        PaperText("How fast sound travels, and how much treble the air takes from a distant sound.", 14, Muted)
    }

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

/** The coldest and warmest air a place can have (°C). The engine's limits. */
private const val ColdestAir = -20f
private const val WarmestAir = 50f

/** A sound closer than this to where it was placed counts as standing still (m). */
private const val StillWithin = .05f
