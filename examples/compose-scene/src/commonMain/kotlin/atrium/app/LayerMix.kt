package atrium.app

/**
 * Which layers were silenced by hand, by their ear in the layer list, and which are being heard
 * [alone] for a moment. While any are heard alone every other layer is silent and those sound,
 * whatever their ears say; when none are, the ears count again.
 */
internal data class LayerMix(val muted: Set<String> = emptySet(), val alone: Set<String> = emptySet()) {
    /** Whether the sound [id] is silent. */
    fun silences(id: String) = if (alone.isEmpty()) id in muted else id !in alone

    /** Hear only [ids] until this is called with none. */
    fun hearingAlone(ids: Set<String>) = copy(alone = ids)

    fun toggleMute(id: String) = copy(muted = if (id in muted) muted - id else muted + id)

    /** Mute every sound in [ids], or let them all sound again if they are all muted already. */
    fun toggleMuteAll(ids: Collection<String>) =
        copy(muted = if (muted.containsAll(ids)) muted - ids.toSet() else muted + ids)

    /** Forget [id], when its sound is taken out of the scene. */
    fun without(id: String) = copy(muted = muted - id, alone = alone - id)
}

/** The sounds of a scene in their groups, each group where its first sound stands in the scene. */
internal fun <T> groupedInSceneOrder(sounds: List<T>, groupOf: (T) -> String?): List<Pair<String, List<T>>> =
    sounds.groupBy { groupOf(it) ?: UngroupedLayers }.toList()

/** The heading for sounds whose scene gives them no group. */
internal const val UngroupedLayers = "Other"

/** The quietest and loudest level a layer's slider reaches, in dB SPL at 1 m. */
internal const val QuietestLayerLevel = 20f
internal const val LoudestLayerLevel = 100f
