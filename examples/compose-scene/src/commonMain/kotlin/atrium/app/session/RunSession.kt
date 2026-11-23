package atrium.app.session

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.withFrameMillis
import atrium.engine.Engine
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext

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
            runCatching {
                val new = request.new
                if (new == null) Engine(request.scene.path) else Engine.newScene(new.kind)
            }
        }
        val running = started.getOrElse { failure ->
            session.failure = failure.message ?: failure.toString()
            session.status = "${request.scene.title} did not start · ${session.failure}"
            return@LaunchedEffect
        }
        try {
            session.forgetMix()
            session.tracks.clear()
            session.analysis.restart(running.analysisScale())
            session.unsaved = false
            session.weather = running.weather()
            session.probe = running.probe()
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
            var space = session.description?.space
            while (true) {
                val frame = withFrameMillis { it }
                // A new place builds the scene again, and its sounds may take other voices: what the
                // tracks heard under each voice no longer belongs to it.
                val placed = session.description?.space
                if (placed != space) {
                    space = placed
                    session.tracks.clear()
                }
                if (frame - lastPlanned >= PlannedEveryMillis) {
                    session.planned = running.planned(PlannedAhead)
                    lastPlanned = frame
                }
                running.latestTelemetry()?.let {
                    session.telemetry = it
                    if (!session.paused) session.tracks.record(running.sceneTime().toDouble(), it.sources)
                }
                // Read whichever tab shows, so the analysis has the past when it is opened; the
                // engine drops what is not read within a few seconds. While paused the columns
                // carry the time the clock stopped at, and the history leaves them out.
                session.analysis.record(running.analysis())
            }
        } finally {
            session.engine = null
            session.description = null
            session.planned = emptyList()
            session.probe = null
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
