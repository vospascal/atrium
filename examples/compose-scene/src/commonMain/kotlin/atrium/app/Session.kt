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
import atrium.engine.Position
import atrium.engine.Preset
import atrium.engine.RenderMode
import atrium.engine.SceneDescription
import atrium.engine.SceneFile
import atrium.engine.SourceDescription
import atrium.engine.Telemetry
import atrium.engine.listPresets
import atrium.engine.listScenes
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.time.Duration.Companion.seconds
import kotlin.time.TimeSource

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

    /** What every voice did over the last two minutes, for the tracks under the map. */
    val tracks = TrackHistory()

    /** What the listener silenced or asked to hear alone; forgotten when the scene changes. */
    var mix by mutableStateOf(LayerMix())
        private set

    /** Whether the playing scene was changed since it was opened or last saved. */
    var unsaved by mutableStateOf(false)
        internal set
    /** Why the last save did not work; null once one does. */
    var saveFailure by mutableStateOf<String?>(null)
        private set
    /** Counts saves, so that lists of scene files are read again after one. */
    var saves by mutableStateOf(0)
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
        unsaved = true
    }

    /** Rests asked for while a handle is dragged; one coroutine applies the newest for each sound. */
    private val wantedGaps = Channel<Triple<UShort, Float, Float>>(Channel.UNLIMITED)
    private var applyingGaps = false

    /**
     * Let the sound in [slot] rest between [shortest] and [longest] seconds between takes; both 0
     * is all the time. Switching between the two builds the sound again, which takes a moment, so
     * this runs off the UI thread, and while a handle is dragged only the newest value is sent.
     */
    fun gap(slot: UShort, shortest: Float, longest: Float) {
        unsaved = true
        wantedGaps.trySend(Triple(slot, shortest, longest))
        if (applyingGaps) return
        applyingGaps = true
        building.launch {
            for (first in wantedGaps) {
                val newest = mutableMapOf(first.first to first)
                while (true) {
                    val more = wantedGaps.tryReceive().getOrNull() ?: break
                    newest[more.first] = more
                }
                val running = engine ?: continue
                for ((slotWanted, shortestWanted, longestWanted) in newest.values) {
                    runCatching { running.setSourceGap(slotWanted, shortestWanted, longestWanted) }
                }
                if (engine === running) description = running.scene()
            }
        }
    }

    /** Let the sound in [slot] utter one take [fewest] to [most] times running before choosing another. */
    fun bout(slot: UShort, fewest: Int, most: Int) {
        val running = engine ?: return
        command { setSourceBout(slot, fewest.toUInt(), most.toUInt()) }
        description = running.scene()
        unsaved = true
    }

    /** The take being heard on its own, as the sound's slot and the take, and when it was asked for. */
    private var heardTake by mutableStateOf<Pair<UShort, Int>?>(null)
    private var heardSince = TimeSource.Monotonic.markNow()

    /** Hear take [take] of the sound in [slot] on its own, once, just ahead of the listener; null stops it. */
    fun hearTake(slot: UShort, take: Int?) {
        val running = engine ?: return
        heardTake = take?.let { slot to it }
        heardSince = TimeSource.Monotonic.markNow()
        building.launch {
            libraryFailure = runCatching { running.auditionTake(slot, take?.toUShort()) }.exceptionOrNull()?.message
        }
    }

    /**
     * Whether take [take] of the sound in [slot] is being heard on its own: asked for, and either
     * still being built or sounding from the audition voice.
     */
    fun isHearing(slot: UShort, take: Int): Boolean {
        if (heardTake != (slot to take)) return false
        val sounding = telemetry?.sources?.any { it.slot.toInt() == AuditionVoice && it.take != null } == true
        return sounding || heardSince.elapsedNow() < BuildingTakes
    }

    /** How often take [take] of the sound in [slot] is chosen next to its others; 0 is never. */
    fun chance(slot: UShort, take: Int, chance: Float) {
        val running = engine ?: return
        command { setTakeChance(slot, take.toUShort(), chance) }
        description = running.scene()
        unsaved = true
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
                        unsaved = true
                        onAdded(added.id)
                    }
                }
                .onFailure { libraryFailure = it.message }
        }
    }

    /** Move the sound in [slot] to [position] while it is dragged. */
    fun move(slot: UShort, position: Position) {
        command { setSourcePosition(slot, position) }
        unsaved = true
    }

    /** Turn the sound in [slot] to face along [facing] while its handle is dragged. */
    fun face(slot: UShort, facing: Position) {
        command { setSourceFacing(slot, facing) }
        unsaved = true
    }

    /** Make the sound in [slot] go out [width] degrees wide, with [behind] dB heard behind it. */
    fun beam(slot: UShort, width: Float, behind: Float) {
        val running = engine ?: return
        command { setSourceBeam(slot, width, behind) }
        description = running.scene()
        unsaved = true
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
        unsaved = true
    }

    /**
     * Write the playing scene to [path] under [title]. From then on the scene that plays is that
     * file, without restarting it, so the next save goes there too.
     */
    fun save(path: String, title: String) {
        val running = engine ?: return
        val playing = request ?: return
        try {
            running.saveScene(path, title)
        } catch (refused: EngineException) {
            saveFailure = refused.message
            return
        }
        saveFailure = null
        unsaved = false
        saves++
        val directory = path.substringBeforeLast('/', ".")
        val written = runCatching { listScenes(directory) }.getOrNull()?.firstOrNull { it.path == path }
        if (written != null) request = playing.copy(scene = written)
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
    // Keyed on the request's number, not the request: saving under a new name changes which
    // file the request names, and that must not restart the scene.
    LaunchedEffect(request?.number) {
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
            session.tracks.clear()
            session.unsaved = false
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
                running.latestTelemetry()?.let {
                    session.telemetry = it
                    session.tracks.record(trackSeconds(), it.sources)
                }
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

/** The voice a preset or a single take is heard from on its own: the engine's last. */
private const val AuditionVoice = 15

/** How long building a take to hear it may take before it is heard (it decodes a recording). */
private val BuildingTakes = 1.5.seconds

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
