package atrium.app.session

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import atrium.engine.Engine
import atrium.engine.EngineException
import atrium.engine.LabCandidate
import atrium.engine.LabFader
import atrium.engine.LabPassage
import atrium.engine.LabRecording
import atrium.engine.LabRun
import atrium.engine.LabSettingKind
import atrium.engine.LabState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * The generator lab as the app holds it: the engine's lab state, read every frame, and what only
 * the screen knows, the recording's and the run's waveforms and the markers while they are dragged.
 *
 * Opening a recording, running the wind from its seed and building the passage take a moment, so
 * they run on [work], one at a time, off the UI thread; [busy] says what is under way. Everything
 * else is a quick call. [resumeScene] lets the scene's clock run, which the lab's voice plays on.
 */
@Stable
internal class LabBench(
    private val work: CoroutineScope,
    private val engineOf: () -> Engine?,
    private val resumeScene: () -> Unit,
) {
    /** The engine's lab, as of the last frame; null with nothing playing. */
    var state by mutableStateOf<LabState?>(null)
        private set
    var recording by mutableStateOf<LabRecording?>(null)
        private set
    var run by mutableStateOf<LabRun?>(null)
        private set

    /** The markers as dragged: the passage in the recording, and its start in the seeded run (s). */
    var referenceFrom by mutableStateOf(0f)
    var referenceTo by mutableStateOf(0f)
    var generatorCue by mutableStateOf(0f)
    var passageName by mutableStateOf("Gust 1")

    /** What is being made, while something is. */
    var busy by mutableStateOf<String?>(null)
        private set
    /** Why the last thing asked for did not happen; null once one does. */
    var failure by mutableStateOf<String?>(null)
        private set
    /** Where the session was last saved. */
    var savedTo by mutableStateOf<String?>(null)
        private set

    /** Whether the Lab has the keys: space plays and pauses it, R, P and W switch. */
    var focused by mutableStateOf(false)
    /** The Behaviour panel starts closed. */
    var behaviourOpen by mutableStateOf(false)

    private var engineSeen: Engine? = null

    /** How long the passage is: the recording's stretch between its markers (s). */
    val passageSeconds: Float get() = (referenceTo - referenceFrom).coerceAtLeast(0f)

    /** Read the engine's lab; a new scene starts an empty one. Called every frame while the Lab shows. */
    fun refresh() {
        val engine = engineOf()
        if (engine !== engineSeen) {
            engineSeen = engine
            recording = null
            run = null
            failure = null
            savedTo = null
        }
        state = engine?.labState()
    }

    fun openRecording(file: String) = make("Opening the recording…") { engine ->
        val opened = engine.labOpenRecording(file)
        recording = opened
        val seconds = opened.waveform.seconds
        referenceFrom = (seconds * .4f).coerceAtLeast(0f)
        referenceTo = (referenceFrom + DefaultPassage).coerceAtMost(seconds)
        if (run == null) runFrom(engine, state?.seed ?: 1u)
    }

    /** Run the working wind from [seed] and draw it; a marked passage plays from it. */
    fun seed(seed: UInt) = make("Running the wind from seed $seed…") { engine -> runFrom(engine, seed) }

    private fun runFrom(engine: Engine, seed: UInt) {
        val made = engine.labGeneratorRun(seed)
        run = made
        if (state?.passage == null) generatorCue = made.moves.firstOrNull()?.seconds ?: 0f
    }

    /** Name and keep the markers' passage, and loop it. */
    fun markPassage() = make("Running the wind to the cue…") { engine ->
        engine.labMarkPassage(LabPassage(passageName, referenceFrom, referenceTo, generatorCue))
    }

    fun togglePlay() {
        val playing = state?.playing == true
        if (!playing) resumeScene()
        make(null) { engine -> engine.labPlay(!playing) }
    }

    /** Start the marked passage again from its cues, playing. */
    fun replay() {
        resumeScene()
        make(null) { engine -> engine.labReplay() }
    }

    fun loop(looping: Boolean) = call { labSetLooping(looping) }

    fun hear(candidate: LabCandidate) = call { labHear(candidate) }

    fun fader(fader: LabFader) = call { labSetFader(fader) }

    fun undoMix() = call { labUndoMix() }

    /**
     * Set the working wind's setting [name]. A level or colour setting is heard at once; a timing
     * setting runs the wind from its seed again, which takes a moment.
     */
    fun setting(name: String, value: Float, kind: LabSettingKind) {
        if (kind == LabSettingKind.TIMING) make("Running the wind to the cue…") { it.labSetSetting(name, value) }
        else call { labSetSetting(name, value) }
    }

    fun runFreely(runFreely: Boolean) = make("Running the wind…") { it.labSetRunFreely(runFreely) }

    /** Measure each candidate alone and trim the generators to the reference. */
    fun matchLevels() {
        resumeScene()
        make(null) { it.labMatchLevels() }
    }

    fun holdLevelMatch(hold: Boolean) = call { labSetHoldLevelMatch(hold) }

    fun pinNote(text: String) = call { labPinNote(text) }

    fun question(text: String) = call { labSetQuestion(text) }

    fun verdict(text: String) = call { labSetVerdict(text) }

    fun save() = call { savedTo = labSave() }

    /** Open the saved session again, with its recording, passage, seed and settings. */
    fun reopen() = make("Reopening the session…") { engine ->
        val reopened = engine.labReopen()
        recording = reopened.recording
        run = reopened.run
        engine.labState().passage?.let { passage ->
            passageName = passage.name
            referenceFrom = passage.referenceFrom
            referenceTo = passage.referenceTo
            generatorCue = passage.generatorCue
        }
    }

    private fun call(action: Engine.() -> Unit) {
        val engine = engineOf() ?: return
        failure = try {
            engine.action()
            null
        } catch (refused: EngineException) {
            refused.message
        }
    }

    private fun make(label: String?, action: (Engine) -> Unit) {
        val engine = engineOf() ?: return
        work.launch {
            if (label != null) busy = label
            failure = try {
                action(engine)
                null
            } catch (refused: EngineException) {
                refused.message
            } finally {
                if (label != null) busy = null
            }
        }
    }

    companion object {
        /** How long a passage the markers first hold (s): a gust with its lead-in and tail. */
        const val DefaultPassage = 7f
    }
}
