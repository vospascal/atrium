package atrium.app.editor

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import atrium.app.session.Session
import atrium.app.ui.Chip
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.engine.Probe

/**
 * Listening to one part of what the engine renders at a time: the direct sound, the
 * reflections, the late reverb or the ambient sounds, with single effects of the air and the
 * ground on or off, from every output or one loudspeaker or ear alone.
 *
 * Nothing else about the scene changes, so the parts heard alone add up to the whole. It is
 * yours, like a mute: it is not saved with the scene, and undo leaves it alone.
 */
internal val WholeRender = Probe(
    direct = true,
    reflections = true,
    reverb = true,
    ambient = true,
    airAbsorption = true,
    movingAir = true,
    groundEffect = true,
    wallColouring = true,
    onlyChannel = null,
)

/** Whether [this] is the scene as it is, with nothing hidden or switched off. */
internal val Probe.isWhole: Boolean get() = this == WholeRender

/** A part of the render that can be heard alone. */
internal enum class RenderPart(val label: String, val said: String) {
    Direct("Direct", "direct sound"),
    Reflections("Reflections", "reflections"),
    Reverb("Reverb", "reverb"),
    Ambient("Ambient", "ambient sound"),
    ;

    /** [probe] hearing this part and no other; its effects and its output stay as they were. */
    fun alone(probe: Probe): Probe = probe.copy(
        direct = this == Direct,
        reflections = this == Reflections,
        reverb = this == Reverb,
        ambient = this == Ambient,
    )

    /** Whether [probe] lets this part through. */
    fun isHeard(probe: Probe): Boolean = when (this) {
        Direct -> probe.direct
        Reflections -> probe.reflections
        Reverb -> probe.reverb
        Ambient -> probe.ambient
    }

    companion object {
        /** The one part [probe] lets through, or null when it lets through all of them or some other mix. */
        fun heardAlone(probe: Probe): RenderPart? = entries.firstOrNull { it.alone(probe) == probe }
    }
}

/** [this] hearing every part again; its effects and its output stay as they were. */
internal fun Probe.everyPart(): Probe = copy(direct = true, reflections = true, reverb = true, ambient = true)

/** Whether [this] lets every part through. */
internal val Probe.hearsEveryPart: Boolean get() = direct && reflections && reverb && ambient

/** An effect of the air or the ground that can be switched off, to hear what it does. */
internal enum class RenderEffect(val label: String, val said: String) {
    AirAbsorption("Air absorption", "air absorption"),
    MovingAir("Moving air", "moving air"),
    Ground("Ground", "ground effect"),
    Walls("Wall colour", "wall colour"),
    ;

    fun isOn(probe: Probe): Boolean = when (this) {
        AirAbsorption -> probe.airAbsorption
        MovingAir -> probe.movingAir
        Ground -> probe.groundEffect
        Walls -> probe.wallColouring
    }

    fun toggled(probe: Probe): Probe = when (this) {
        AirAbsorption -> probe.copy(airAbsorption = !probe.airAbsorption)
        MovingAir -> probe.copy(movingAir = !probe.movingAir)
        Ground -> probe.copy(groundEffect = !probe.groundEffect)
        Walls -> probe.copy(wallColouring = !probe.wallColouring)
    }
}

/**
 * What [probe] leaves of the scene, in a line for the top bar: "Hearing reverb only · without
 * moving air · Ls only". Null when it is the whole scene. [channels] names the output channels.
 */
internal fun probeLine(probe: Probe, channels: List<String>): String? {
    if (probe.isWhole) return null
    val part = RenderPart.heardAlone(probe)
    val parts = when {
        probe.hearsEveryPart -> null
        part != null -> "${part.said} only"
        else -> RenderPart.entries.filter { it.isHeard(probe) }.joinToString(" and ") { it.said }.ifEmpty { "nothing" }
    }
    val off = RenderEffect.entries.filterNot { it.isOn(probe) }.joinToString(", ") { it.said }.ifEmpty { null }?.let { "without $it" }
    val output = probe.onlyChannel?.let { "${channels.getOrNull(it.toInt()) ?: "channel ${it.toInt() + 1}"} only" }
    return "Hearing " + listOfNotNull(parts, off, output).joinToString(" · ")
}

/** The Listen part of the scene inspector. */
@Composable
internal fun ListenPart(session: Session) {
    val probe = session.probe ?: return
    val channels = outputNames(session)
    val spaced = Arrangement.spacedBy(8.dp)

    PaperText("Hear only", 14, Muted)
    Spacer(Modifier.height(6.dp))
    FlowRow(horizontalArrangement = spaced, verticalArrangement = spaced) {
        Chip("Everything", selected = probe.hearsEveryPart) { session.listenTo(probe.everyPart()) }
        RenderPart.entries.forEach { part ->
            Chip(part.label, selected = RenderPart.heardAlone(probe) == part) { session.listenTo(part.alone(probe)) }
        }
    }

    Spacer(Modifier.height(12.dp))
    PaperText("With", 14, Muted)
    Spacer(Modifier.height(6.dp))
    FlowRow(horizontalArrangement = spaced, verticalArrangement = spaced) {
        RenderEffect.entries.forEach { effect ->
            Chip(effect.label, selected = effect.isOn(probe)) { session.listenTo(effect.toggled(probe)) }
        }
    }

    if (channels.size > 1) {
        Spacer(Modifier.height(12.dp))
        PaperText("From", 14, Muted)
        Spacer(Modifier.height(6.dp))
        FlowRow(horizontalArrangement = spaced, verticalArrangement = spaced) {
            Chip("Every output", selected = probe.onlyChannel == null) { session.listenTo(probe.copy(onlyChannel = null)) }
            channels.forEachIndexed { channel, name ->
                Chip(name, selected = probe.onlyChannel?.toInt() == channel) { session.listenTo(probe.copy(onlyChannel = channel.toUByte())) }
            }
        }
    }

    Spacer(Modifier.height(8.dp))
    PaperText("The parts heard alone add up to the whole. This is yours, like a mute: it is not saved with the scene.", 14, Muted)
}

/** What the output channels playing now are called: the room's loudspeakers, or L and R on headphones. */
internal fun outputNames(session: Session): List<String> {
    val telemetry = session.telemetry ?: return emptyList()
    return channelNames(
        telemetry.peaksDb.size,
        telemetry.channelsInUse.isEmpty(),
        session.description?.speakers.orEmpty().associate { it.channel.toInt() to it.name },
    )
}
