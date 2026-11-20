package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import atrium.engine.ChannelMode
import atrium.engine.Engine
import atrium.engine.EngineException
import atrium.engine.HeadTracker
import atrium.engine.Output
import atrium.engine.Pattern
import atrium.engine.Position
import atrium.engine.Preset
import atrium.engine.RenderMode
import atrium.engine.SceneDescription
import atrium.engine.SceneFile
import atrium.engine.SourceDescription
import atrium.engine.Telemetry
import atrium.engine.listPresets
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The one engine the app runs, shared by the player and the editor.
 *
 * Screens read its state and call [play], [stop] or the running [engine];
 * [RunSession] starts and closes engines to match.
 */
@Stable
internal class Session(private val store: SettingsStore) {
    /** The scene asked for; a new request restarts even the same scene. */
    var request by mutableStateOf<PlayRequest?>(null)
        private set
    var engine by mutableStateOf<Engine?>(null)
        internal set
    var description by mutableStateOf<SceneDescription?>(null)
        internal set
    var outputs by mutableStateOf<List<Output>>(emptyList())
        internal set
    /** The newest frame; kept between frames so the screens never blank. */
    var telemetry by mutableStateOf<Telemetry?>(null)
        internal set
    var status by mutableStateOf("Nothing playing")
        internal set
    /** Why the requested scene did not start; null while starting or playing. */
    var failure by mutableStateOf<String?>(null)
        internal set
    /** The head tracker asked for. It outlives a scene: each engine gets it as it starts. */
    var headTracker by mutableStateOf<HeadTracker?>(null)
        private set
    /** Why the chosen tracker did not start; null while it runs or is off. */
    var headTrackingFailure by mutableStateOf<String?>(null)
        private set
    /**
     * The output last chosen in Settings. It belongs to this device and not to a scene, so it
     * outlives a scene and a restart: every scene starts on it when it can be played here.
     */
    var chosenOutput by mutableStateOf(outputSaved(store.read(OutputKey)))
        private set

    /** What the listener silenced or asked to hear alone; forgotten when the scene changes. */
    var mix by mutableStateOf(LayerMix())
        private set

    /** The presets a sound can be made from, read when the library is first opened. */
    val presets: Result<List<Preset>> by lazy { runCatching { listPresets(PresetDirectory) } }
    /** Why the last audition or add did not work; null once one does. */
    var libraryFailure by mutableStateOf<String?>(null)
        private set

    /** Building a sound decodes or measures it, which takes a moment: one at a time, off the UI thread. */
    private val building = CoroutineScope(SupervisorJob() + Dispatchers.Default.limitedParallelism(1))

    private var requests = 0

    fun play(scene: SceneFile) {
        request = PlayRequest(scene, ++requests)
    }

    fun stop() {
        request = null
    }

    /** Whether [scene] is the one asked for: starting, playing, or failed to start. */
    fun isActive(scene: SceneFile) = request?.scene?.path == scene.path

    /** Make [next] the layer mix, telling the engine about every sound that falls silent or returns. */
    fun mix(next: LayerMix) {
        val before = mix
        mix = next
        description?.sources?.forEach { sound ->
            val silent = next.silences(sound.id)
            if (silent != before.silences(sound.id)) command { setSourceMuted(sound.slot, silent) }
        }
    }

    /** Set the level of the sound in [slot], in dB SPL at 1 m. */
    fun level(slot: UShort, level: Float) {
        val running = engine ?: return
        command { setSourceLevel(slot, level) }
        description = running.scene()
    }

    /** Play [preset] just ahead of the listener to hear it before placing it, or stop with null. */
    fun audition(preset: String?) {
        val running = engine ?: return
        building.launch {
            libraryFailure = runCatching { running.audition(preset) }.exceptionOrNull()?.message
        }
    }

    /** Add a sound made from [preset] to the playing scene; [onAdded] gets its id once it sounds. */
    fun add(preset: String, onAdded: (id: String) -> Unit) {
        val running = engine ?: return
        building.launch {
            runCatching { running.addSound(preset) }
                .onSuccess { added ->
                    libraryFailure = null
                    if (engine === running) {
                        description = running.scene()
                        onAdded(added.id)
                    }
                }
                .onFailure { libraryFailure = it.message }
        }
    }

    /** Move the sound in [slot] to [position] while it is dragged. */
    fun move(slot: UShort, position: Position) = command { setSourcePosition(slot, position) }

    /** Turn the sound in [slot] to face along [facing] while its handle is dragged. */
    fun face(slot: UShort, facing: Position) = command { setSourceFacing(slot, facing) }

    /** Change how the sound in [slot] radiates. */
    fun shape(slot: UShort, pattern: Pattern) {
        val running = engine ?: return
        command { setSourcePattern(slot, pattern) }
        description = running.scene()
    }

    /**
     * Take [sound] out of the playing scene. It is also dropped from the layer mix first, so
     * that removing a layer that was heard alone lets the others sound again.
     */
    fun remove(sound: SourceDescription) {
        val running = engine ?: return
        mix(mix.without(sound.id))
        command { removeSound(sound.slot) }
        description = running.scene()
    }

    /** A drag has ended: take over where the engine now says the scene's sounds were placed. */
    fun placed() {
        description = engine?.scene() ?: return
    }

    /** A new scene starts with every layer sounding. */
    internal fun forgetMix() {
        mix = LayerMix()
    }

    /** Play on [output] from now on: the scene that plays, and every scene started after it. */
    fun chooseOutput(output: Output) {
        chosenOutput = output
        store.write(OutputKey, output.saved())
        command { setOutput(output) }
    }

    /** Switch a scene that just started to the chosen output, when it can be played here. */
    internal fun followChosenOutput() {
        val chosen = chosenOutput?.takeIf { it in outputs } ?: return
        command { setOutput(chosen) }
    }

    /** Let [tracker] turn the listener with the head, or switch it off with null. */
    fun chooseHeadTracker(tracker: HeadTracker?) {
        headTracker = tracker
        engine?.let(::followHeadTracker)
    }

    /** Give [running] the chosen tracker. A failure is kept for Settings to show. */
    internal fun followHeadTracker(running: Engine) {
        headTrackingFailure = try {
            running.setHeadTracker(headTracker)
            null
        } catch (refused: EngineException.HeadTracking) {
            refused.message
        }
    }

    /** Commands are dropped only when the audio thread is behind; the next one catches up. */
    fun command(action: Engine.() -> Unit) {
        val running = engine ?: return
        try {
            running.action()
        } catch (_: EngineException.CommandQueueFull) {
        }
    }
}

internal data class PlayRequest(val scene: SceneFile, val number: Int)

/** Keeps [session]'s engine in step with its request, and its telemetry current. */
@Composable
internal fun RunSession(session: Session) {
    val request = session.request
    LaunchedEffect(request) {
        if (request == null) {
            session.failure = null
            session.status = "Nothing playing"
            return@LaunchedEffect
        }
        session.failure = null
        session.status = "Starting ${request.scene.title}…"
        // Starting blocks while audio is decoded and the device opens. It must
        // not be abandoned half-way, or the started engine would play on
        // until garbage collection; so it always completes and is closed below.
        val started = withContext(Dispatchers.Default + NonCancellable) {
            runCatching { Engine(request.scene.path) }
        }
        val running = started.getOrElse { failure ->
            session.failure = failure.message ?: failure.toString()
            session.status = "${request.scene.title} did not start · ${session.failure}"
            return@LaunchedEffect
        }
        try {
            session.forgetMix()
            session.description = running.scene()
            session.outputs = running.outputs()
            session.engine = running
            session.status = "Playing ${request.scene.title}"
            session.followChosenOutput()
            // The engine that played before has closed by now and let go of the
            // tracker's port, so this one can take it.
            if (session.headTracker != null) session.followHeadTracker(running)
            while (true) {
                withFrameMillis { }
                running.latestTelemetry()?.let { session.telemetry = it }
            }
        } finally {
            session.engine = null
            session.description = null
            session.outputs = emptyList()
            session.telemetry = null
            running.close()
        }
    }
}

/** Where the presets are, relative to the working directory, like the scenes. */
private const val PresetDirectory = "sources"

private const val OutputKey = "output"

/** An output as the settings store keeps it, such as `HRTF/STEREO`. */
internal fun Output.saved(): String = "${renderMode.name}/${channelMode.name}"

/** The output [saved] names, or null when nothing was saved or it names an output this version does not have. */
internal fun outputSaved(saved: String?): Output? {
    val names = saved?.split('/')?.takeIf { it.size == 2 } ?: return null
    val renderMode = RenderMode.entries.firstOrNull { it.name == names[0] } ?: return null
    val channelMode = ChannelMode.entries.firstOrNull { it.name == names[1] } ?: return null
    return Output(renderMode, channelMode)
}
