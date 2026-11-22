package atrium.app.editor

import atrium.engine.SourceDescription

/**
 * Selected loops brought onto one playhead: the loop they line up to, and the loops that jump to
 * it. A loop starts at its own beginning when it is added from the library or copied, so it plays
 * out of step with the loops already playing until it is brought into step.
 */
internal data class InStep(val anchor: SourceDescription, val joining: List<SourceDescription>)

/**
 * What playing [selection] in step would do among the scene's [sounds], or null when fewer than two
 * of the selected sounds loop and there is nothing to bring into step.
 *
 * The first selected sound that loops leads: the one picked first is the one the others were picked
 * to match. A sound that rests between takes or is generated has no playhead, so it neither leads
 * nor joins, even when it was picked first.
 */
internal fun inStep(selection: Selection, sounds: List<SourceDescription>): InStep? {
    val byId = sounds.associateBy { it.id }
    val loops = selection.ids.mapNotNull { byId[it] }.filter { it.loops }
    if (loops.size < 2) return null
    return InStep(loops.first(), loops.drop(1))
}
