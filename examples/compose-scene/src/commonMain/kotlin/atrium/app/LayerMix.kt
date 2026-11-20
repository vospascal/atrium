package atrium.app

/** Which layers were silenced by hand, by their ear in the layer list. */
internal data class LayerMix(val muted: Set<String> = emptySet()) {
    /** Whether the sound [id] is silent. */
    fun silences(id: String) = id in muted

    fun toggleMute(id: String) = copy(muted = if (id in muted) muted - id else muted + id)

    /** Mute every sound in [ids], or let them all sound again if they are all muted already. */
    fun toggleMuteAll(ids: Collection<String>) =
        copy(muted = if (muted.containsAll(ids)) muted - ids.toSet() else muted + ids)

    /** Forget [id], when its sound is taken out of the scene. */
    fun without(id: String) = copy(muted = muted - id)
}

/** The sounds of a scene in their groups, each group where its first sound stands in the scene. */
internal fun <T> groupedInSceneOrder(sounds: List<T>, groupOf: (T) -> String?): List<Pair<String, List<T>>> =
    sounds.groupBy { groupOf(it) ?: UngroupedLayers }.toList()

/** The heading for sounds whose scene gives them no group. */
internal const val UngroupedLayers = "Other"

/** The quietest and loudest level a layer's slider reaches, in dB SPL at 1 m. */
internal const val QuietestLayerLevel = 20f
internal const val LoudestLayerLevel = 100f
