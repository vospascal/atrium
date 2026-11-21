package atrium.app.session

import kotlin.time.TimeSource

/**
 * Which edits belong to one step of undo.
 *
 * A slider dragged or a sound moved sends many edits a second, and undo should take all of them
 * back at once, as one step. Edits of the same kind to the same thing, each soon after the one
 * before, are one step; anything else starts a new one. This is how a text editor groups typing,
 * and it needs nothing from the controls that make the edits.
 */
internal class Steps(private val clock: () -> Long = monotonicMillis()) {
    private var lastKey: String? = null
    private var lastAt = 0L

    /** Whether an edit known by [key] (such as "level:3") starts a new step now. */
    fun starts(key: String): Boolean {
        val at = clock()
        val starts = key != lastKey || at - lastAt > SameStepWithinMillis
        lastKey = key
        lastAt = at
        return starts
    }

    /** The next edit starts a new step, whatever it is: after an undo, nothing joins the step before. */
    fun end() {
        lastKey = null
    }
}

private fun monotonicMillis(): () -> Long {
    val start = TimeSource.Monotonic.markNow()
    return { start.elapsedNow().inWholeMilliseconds }
}

/** Edits further apart than this are separate steps, even of the same thing. */
internal const val SameStepWithinMillis = 1000L
