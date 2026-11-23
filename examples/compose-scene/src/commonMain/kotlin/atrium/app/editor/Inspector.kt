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
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.FlowRow
import atrium.app.ui.Chip
import atrium.engine.SceneFile
import atrium.engine.SceneKind
import atrium.engine.SourceDescription
import kotlin.math.roundToInt
import atrium.app.ui.Muted
import atrium.app.ui.PaperLevel
import atrium.app.ui.PaperText
import atrium.app.Point
import atrium.app.session.EnvironmentDirectory
import atrium.app.session.Session
import atrium.app.session.VoiceCount
import atrium.engine.listPlaces
import androidx.compose.runtime.remember
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
            chosen.size > 1 -> SeveralInspector(session, chosen, inStep(selection, sounds))
            chosen.size == 1 -> SoundInspector(session, chosen.single())
            selection.single == ListenerId -> ListenerInspector(session)
            else -> SceneInspector(session, scene, sounds.size)
        }
    }
}

/** Several sounds selected: which, and what applies to all of them at once; [inStep] when two or more loop. */
@Composable
private fun SeveralInspector(session: Session, sounds: List<SourceDescription>, inStep: InStep?) {
    PaperText("${sounds.size} sounds", 26, weight = FontWeight.Bold, modifier = Modifier.padding(top = 22.dp))
    PaperText("Select one of them alone to change its sound, shape or rhythm.", 14, Muted)

    Part("Selected")
    sounds.forEach { PaperText(it.name, 15) }

    Part("Together")
    PaperText("Drag one on the map and they all move; the arrow keys nudge them.", 14, Muted)
    PaperText("Hold Alt while dragging to leave copies where you let go.", 14, Muted)
    PaperText("Hold S to hear only these. ⌘D duplicates them; the bin under the layers removes them.", 14, Muted)
    if (inStep != null) {
        Spacer(Modifier.height(14.dp))
        PaperText("A loop added or copied later starts at its own beginning. This puts the others on ${inStep.anchor.name}'s playhead.", 14, Muted)
        Spacer(Modifier.height(8.dp))
        ToolButton("Play in step with ${inStep.anchor.name}") { session.playInStep(inStep.anchor, inStep.joining) }
    }

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
    LayersPart(session, sound)

    Part("Shape")
    ShapePart(session, sound, live)

    Part("Place")
    PaperText(awayLabel(offset), 15)
    PaperText("${oneDecimal(at.z)} m above the ground", 14, Muted)

    Part("When")
    WhenPart(session, sound, live)

    if (session.shows(ScenePart.Follows)) {
        Part("Follows")
        FollowsPart(sound.follows)
    }

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
    if (description != null && session.shows(ScenePart.Kind)) {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            SceneKind.entries.forEach { kind -> Chip(kind.label, selected = kind == description.kind) { session.sceneKind(kind) } }
        }
        Spacer(Modifier.height(8.dp))
    }
    if (description != null && session.shows(ScenePart.Place)) {
        PlacePart(session, description.space)
        Spacer(Modifier.height(8.dp))
    }
    if (session.shows(ScenePart.Voices)) PaperText("$voices of $VoiceCount voices", 14, Muted)

    if (session.shows(ScenePart.Weather)) {
        Part("Weather")
        WeatherPart(session)
    }

    if (description != null && session.shows(ScenePart.Air)) {
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

    if (!session.shows(ScenePart.Listening)) return
    Part("Listen")
    ListenPart(session)

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

/**
 * The place the scene plays in, [current], among Dry and every environment file, with a line on the
 * one chosen. Choosing another builds the scene again there, with a short gap in the sound.
 */
@Composable
private fun PlacePart(session: Session, current: String?) {
    val places = remember { runCatching { listPlaces(EnvironmentDirectory) }.getOrDefault(emptyList()) }
    PaperText("Place", 13, Muted)
    Spacer(Modifier.height(6.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        places.forEach { place -> Chip(spaceLabel(place.space), selected = place.space == current) { session.place(place.space) } }
    }
    Spacer(Modifier.height(6.dp))
    val chosen = places.firstOrNull { it.space == current }
    PaperText(chosen?.let(::placeLine) ?: "${spaceLabel(current)}: the scene file's own environment", 14, Muted)
    session.placeFailure?.let { PaperText(it, 14, Muted) }
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
