package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import atrium.engine.ChannelMode
import atrium.engine.EditHistory
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
import atrium.engine.Conditions
import atrium.engine.PlannedTake
import atrium.engine.Weather
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

    /** Whether the playing scene is paused: its clock, its plan and its weather stand still. */
    var paused by mutableStateOf(false)
        internal set

    /** Pause the playing scene, or play it on from where it stood: its clock, plan and weather stand still meanwhile. */
    fun togglePause() {
        engine ?: return
        val next = !paused
        command { setPaused(next) }
        paused = next
    }

    /** Seconds into the playing scene, as its timeline shows them; 0 with nothing playing. */
    fun sceneTime(): Double = engine?.sceneTime()?.toDouble() ?: 0.0

    /**
     * Move the playing scene to [seconds], as a playhead is dragged: every sound and the weather are
     * as they would be there. What the tracks heard belongs to the old moment, so they start again.
     */
    fun seek(seconds: Double) {
        val running = engine ?: return
        running.seek(seconds.coerceAtLeast(0.0).toFloat())
        tracks.clear()
    }

    /** How fast the weather runs, 1 being as fast as the scene. */
    var weatherSpeed by mutableStateOf(1f)
        private set

    /** Let the weather run [speed] times as fast as the scene; the sounds keep their own pace. */
    fun runWeatherAt(speed: Float) {
        engine?.setWeatherSpeed(speed) ?: return
        weatherSpeed = speed
    }

    /** A new scene starts at the weather's own pace, playing. */
    internal fun resetTransport() {
        paused = false
        weatherSpeed = 1f
    }

    /** The scene's weather story and bands, as the engine has them. */
    var weather by mutableStateOf<Weather?>(null)
        internal set

    /** The takes the plan has decided for the next minute, refreshed a few times a second. */
    var planned by mutableStateOf<List<PlannedTake>>(emptyList())
        internal set

    /** Let [next] move the place's conditions from now on; it is saved with the scene. */
    fun setWeather(next: Weather) {
        step("Weather", "weather")
        command { setWeather(next) }
        weather = next
        unsaved = true
    }

    /** The conditions from [from] to [to] seconds around now, at [steps] moments; empty with nothing playing. */
    fun conditionsAround(from: Float, to: Float, steps: Int): List<Conditions> =
        engine?.conditions(from, to, steps.toUInt()).orEmpty()

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
        step("Level · ${nameOf(slot)}", "level:$slot")
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
        step("Rests · ${nameOf(slot)}", "gap:$slot")
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
        step("Repeats · ${nameOf(slot)}", "bout:$slot")
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
        step("Chance · ${nameOf(slot)}", "chance:$slot:$take")
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

    /**
     * Add a sound made from [preset] to the playing scene, at [at] where it was dropped or else
     * just ahead of the listener; [onAdded] gets its id once it sounds.
     */
    fun add(preset: String, at: Position? = null, onAdded: (id: String) -> Unit) {
        step("Add ${preset.substringAfterLast('/').substringBeforeLast('.')}", "add")
        build(onAdded) { addSound(preset, at) }
    }

    /** Add a copy of [sound] a metre east of it; [onAdded] gets the copy's id once it sounds. */
    fun duplicate(sound: SourceDescription, onAdded: (id: String) -> Unit) {
        step("Duplicate ${sound.name}", "duplicate")
        build(onAdded) { duplicateSound(sound.slot) }
    }

    /** Show [sound] as [name], in [color] (`#rrggbb`, or null for its voice's own) and under [group]. */
    fun identify(sound: SourceDescription, name: String, color: String?, group: String?) {
        val running = engine ?: return
        step("Rename ${sound.name}", "identity:${sound.slot}")
        command { setSourceIdentity(sound.slot, name, color, group) }
        description = running.scene()
        unsaved = true
    }

    /** Build a sound with [adding], off the UI thread, and tell [onAdded] its id once it sounds. */
    private fun build(onAdded: (id: String) -> Unit, adding: Engine.() -> SourceDescription) {
        val running = engine ?: return
        building.launch {
            runCatching { running.adding() }
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
        step("Move · ${nameOf(slot)}", "move:$slot")
        command { setSourcePosition(slot, position) }
        unsaved = true
    }

    /** Move the listener to [position] while it is dragged: every sound is heard from there. */
    fun moveListener(position: Position) {
        step("Move listener", "listener:move")
        command { setListenerPosition(position) }
        unsaved = true
    }

    /** Turn the listener to face along [facing] while its handle is dragged. */
    fun faceListener(facing: Position) {
        step("Turn listener", "listener:face")
        command { setListenerFacing(facing) }
        unsaved = true
    }

    /** Turn the sound in [slot] to face along [facing] while its handle is dragged. */
    fun face(slot: UShort, facing: Position) {
        step("Turn · ${nameOf(slot)}", "face:$slot")
        command { setSourceFacing(slot, facing) }
        unsaved = true
    }

    /** Make the sound in [slot] go out [width] degrees wide, with [behind] dB heard behind it. */
    fun beam(slot: UShort, width: Float, behind: Float) {
        val running = engine ?: return
        step("Beam · ${nameOf(slot)}", "beam:$slot")
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
        step("Remove ${sound.name}", "remove")
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

    /** The steps that can be undone and redone in the playing scene; null with nothing playing. */
    var history by mutableStateOf<EditHistory?>(null)
        internal set

    /** Which edits make one step: a whole drag is undone at once. */
    private val steps = Steps()

    /** Let the engine keep the scene as it is, before an edit that starts a new step called [label]. */
    private fun step(label: String, key: String) {
        val running = engine ?: return
        if (!steps.starts(key)) return
        running.remember(label)
        history = running.history()
    }

    private fun nameOf(slot: UShort) = description?.sources?.firstOrNull { it.slot == slot }?.name ?: "sound"

    /** Take the scene back one step. */
    fun undo() = travel(-1)

    /** Take again the last step undone. */
    fun redo() = travel(1)

    /** Take the scene to entry [index] of its history line, 0 being as it was opened. */
    fun travelTo(index: Int) {
        val at = history?.undoable?.size ?: return
        travel(index - at)
    }

    /**
     * Go [steps] back (negative) or forward. Bringing a sound back builds it again, so this runs
     * off the UI thread, in turn with every other build.
     */
    private fun travel(by: Int) {
        val running = engine ?: return
        if (by == 0) return
        steps.end()
        building.launch {
            for (step in 1..kotlin.math.abs(by)) {
                val moved = runCatching { if (by < 0) running.undo() else running.redo() }.getOrDefault(false)
                if (!moved) break
            }
            if (engine !== running) return@launch
            description = running.scene()
            weather = running.weather()
            history = running.history()
            unsaved = true
            // A sound brought back is built anew, without the listener's mute.
            description?.sources?.forEach { sound ->
                if (mix.silences(sound.id)) command { setSourceMuted(sound.slot, true) }
            }
        }
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
            session.weather = running.weather()
            session.history = running.history()
            session.resetTransport()
            session.description = running.scene()
            session.outputs = running.outputs()
            session.engine = running
            session.status = "Playing ${request.scene.title}"
            session.followChosenOutput()
            // The engine that played before has closed by now and let go of the
            // tracker's port, so this one can take it.
            if (session.headTracker != null) session.followHeadTracker(running)
            var lastPlanned = 0L
            while (true) {
                val frame = withFrameMillis { it }
                if (frame - lastPlanned >= PlannedEveryMillis) {
                    session.planned = running.planned(PlannedAhead)
                    lastPlanned = frame
                }
                running.latestTelemetry()?.let {
                    session.telemetry = it
                    if (!session.paused) session.tracks.record(running.sceneTime().toDouble(), it.sources)
                }
            }
        } finally {
            session.engine = null
            session.description = null
            session.planned = emptyList()
            session.history = null
            session.outputs = emptyList()
            session.telemetry = null
            running.close()
        }
    }
}

/** How far ahead the tracks can show what is planned (s), and how often that is read again. */
internal const val PlannedAhead = 60f
private const val PlannedEveryMillis = 250L

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
