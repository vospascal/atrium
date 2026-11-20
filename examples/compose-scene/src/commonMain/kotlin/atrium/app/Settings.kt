package atrium.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.engine.ChannelMode
import atrium.engine.HeadTracker
import atrium.engine.HeadTrackingStatus
import atrium.engine.RenderMode
import atrium.engine.headTrackers
import kotlin.math.roundToInt

/** How the player looks and how the scene reaches you: appearance, ink, output, head tracking, volume, editor. */
@Composable
internal fun Settings(session: Session, look: Look, onEdit: () -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp)) {
        Box(Modifier.padding(top = 22.dp)) { Wordmark() }
        PaperText("Settings", 52, weight = FontWeight.Bold, modifier = Modifier.padding(top = 24.dp, bottom = 8.dp))

        Section("Appearance")
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Appearance.entries.forEach { entry ->
                Chip(entry.name, selected = entry == look.appearance) { look.appearance = entry }
            }
        }

        Section("Ink")
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Chip("Color", selected = !look.monochrome) { look.monochrome = false }
            Chip("Monochrome", selected = look.monochrome) { look.monochrome = true }
        }

        if (session.engine == null) {
            Spacer(Modifier.height(28.dp))
            PaperText("Play a scene to choose its output and volume.", 16, Muted)
            return@Column
        }

        Section("Output")
        val current = session.telemetry?.output
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            session.outputs.forEach { output ->
                Chip("${output.renderMode.label} · ${output.channelMode.label}", selected = output == current) {
                    session.command { setOutput(output) }
                }
            }
        }

        val trackers = remember { headTrackers() }
        if (trackers.isNotEmpty()) {
            Section("Head tracking")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Chip("Off", selected = session.headTracker == null) { session.chooseHeadTracker(null) }
                trackers.forEach { tracker ->
                    Chip(tracker.label, selected = tracker == session.headTracker) {
                        session.chooseHeadTracker(tracker)
                    }
                }
            }
            Spacer(Modifier.height(10.dp))
            PaperText(
                session.headTrackingFailure ?: when (session.telemetry?.headTracking) {
                    HeadTrackingStatus.RECEIVING ->
                        "Receiving head movement · facing " +
                            "${session.telemetry?.listenerFacing?.let(::bearingLabel)}. The 3D map shows it."
                    HeadTrackingStatus.WAITING ->
                        "Switched on, but no head movement is arriving. " + when (session.headTracker) {
                            HeadTracker.SONY ->
                                "In the Sony app, set Bluetooth Connection Quality to Prioritize Stable " +
                                    "Connection. Then forget the headphones on this Mac and pair them again."
                            else -> "Start your tracker and point it at port 4242 on this machine."
                        }
                    else -> "For headphones. Turn your head and the place stays where it is."
                },
                13,
                Muted,
            )
        }

        Section("Volume")
        // Once touched, the level shows what was set rather than the engine's
        // report from up to a frame interval ago. Nothing else changes the
        // volume, so the two only differ while the engine catches up.
        var held by remember(session.engine) { mutableStateOf<Float?>(null) }
        val volume = held ?: session.telemetry?.volume ?: 0f
        PaperText("${(volume * 100).roundToInt()} %", 15, Muted)
        PaperLevel(volume, onChange = { level ->
            held = level
            session.command { setVolume(level) }
        }, Modifier.fillMaxWidth())

        Section("Edit")
        Chip("Open the editor", selected = false, onClick = onEdit)

        Spacer(Modifier.height(24.dp))
        val peak = session.telemetry?.peaksDb?.maxOrNull()
        PaperText("Peak ${peak?.let { "${it.roundToInt()} dBFS" } ?: "—"}", 13, Muted)
    }
}

@Composable
private fun Section(title: String) {
    Spacer(Modifier.height(28.dp))
    PaperText(title.uppercase(), 13, Muted, FontWeight.SemiBold, spacing = 2f)
    Spacer(Modifier.height(10.dp))
}

internal val RenderMode.label
    get() = when (this) {
        RenderMode.WORLD_LOCKED -> "World-locked"
        RenderMode.VBAP -> "VBAP"
        RenderMode.HRTF -> "Binaural"
        RenderMode.DBAP -> "DBAP"
        RenderMode.AMBISONICS -> "Ambisonics"
    }

private val HeadTracker.label
    get() = when (this) {
        HeadTracker.SONY -> "Sony headphones"
        HeadTracker.EXTERNAL -> "OpenTrack"
    }

internal val ChannelMode.label
    get() = when (this) {
        ChannelMode.STEREO -> "stereo"
        ChannelMode.QUAD -> "quad"
        ChannelMode.SURROUND51 -> "5.1"
    }
