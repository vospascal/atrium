package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import atrium.engine.Engine
import atrium.engine.EngineException
import atrium.engine.HeadTracker
import atrium.engine.Output
import atrium.engine.SceneDescription
import atrium.engine.SceneFile
import atrium.engine.Telemetry
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext

/**
 * The one engine the app runs, shared by the player and the composer.
 *
 * Screens read its state and call [play], [stop] or the running [engine];
 * [RunSession] starts and closes engines to match.
 */
@Stable
internal class Session {
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

    private var requests = 0

    fun play(scene: SceneFile) {
        request = PlayRequest(scene, ++requests)
    }

    fun stop() {
        request = null
    }

    /** Whether [scene] is the one asked for: starting, playing, or failed to start. */
    fun isActive(scene: SceneFile) = request?.scene?.path == scene.path

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
            session.description = running.scene()
            session.outputs = running.outputs()
            session.engine = running
            session.status = "Playing ${request.scene.title}"
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
