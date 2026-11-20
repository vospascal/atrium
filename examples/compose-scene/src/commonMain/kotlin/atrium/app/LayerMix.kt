package atrium.app

/**
 * Which layers were silenced by hand, and which were asked to be heard alone.
 * The engine only knows "muted or not" per sound; this is what the listener
 * asked for, from which that follows.
 */
internal data class LayerMix(val muted: Set<String> = emptySet(), val solo: Set<String> = emptySet()) {
    /** Whether the sound [id] is silent: muted itself, or left out while others are heard alone. */
    fun silences(id: String) = id in muted || (solo.isNotEmpty() && id !in solo)

    fun toggleMute(id: String) = copy(muted = muted.toggled(id))

    fun toggleSolo(id: String) = copy(solo = solo.toggled(id))

    /** Mute every sound in [ids], or let them all sound again if they are all muted already. */
    fun toggleMuteAll(ids: Collection<String>) =
        copy(muted = if (muted.containsAll(ids)) muted - ids.toSet() else muted + ids)

    private fun Set<String>.toggled(id: String) = if (id in this) this - id else this + id
}

/** The sounds of a scene in their groups, each group where its first sound stands in the scene. */
internal fun <T> groupedInSceneOrder(sounds: List<T>, groupOf: (T) -> String?): List<Pair<String, List<T>>> =
    sounds.groupBy { groupOf(it) ?: UngroupedLayers }.toList()

/** The heading for sounds whose scene gives them no group. */
internal const val UngroupedLayers = "Other"

/** The quietest and loudest level a layer's slider reaches, in dB SPL at 1 m. */
internal const val QuietestLayerLevel = 20f
internal const val LoudestLayerLevel = 100f
