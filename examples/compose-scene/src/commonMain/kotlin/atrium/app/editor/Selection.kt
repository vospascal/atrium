package atrium.app.editor

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerKeyboardModifiers
import androidx.compose.ui.input.pointer.isAltPressed
import androidx.compose.ui.input.pointer.isMetaPressed
import androidx.compose.ui.input.pointer.isShiftPressed
import androidx.compose.ui.input.pointer.pointerInput

/**
 * What is selected in the editor: nothing, the listener, or one or more sounds by id.
 *
 * A plain click selects one thing. A click with Shift or ⌘ held adds a sound to the others or
 * takes it out again. The listener is only ever selected alone: it is moved and turned by
 * itself, and nothing done to several sounds at once applies to it.
 */
internal data class Selection(val ids: Set<String> = emptySet()) {
    /** The one thing selected, or null when nothing or several are. */
    val single: String? get() = ids.singleOrNull()

    operator fun contains(id: String) = id in ids

    /** After a click on [id], or on empty ground when it is null; [adding] with Shift or ⌘ held. */
    fun clicked(id: String?, adding: Boolean): Selection = when {
        id == null -> if (adding) this else Selection()
        !adding || id == ListenerId || ListenerId in ids -> Selection(setOf(id))
        id in ids -> Selection(ids - id)
        else -> Selection(ids + id)
    }

    /** After a click on [id] in a list, where a click on the only selected row lets go of it. */
    fun clickedInList(id: String, adding: Boolean): Selection =
        if (!adding && ids == setOf(id)) Selection() else clicked(id, adding)

    /** What a drag that starts on [id] carries: every selected sound when it is one of them, else it alone. */
    fun grabbed(id: String): Selection = if (id in ids && id != ListenerId) this else Selection(setOf(id))

    /** Without what is no longer in the scene, given the ids of the sounds [present]. */
    fun among(present: Collection<String>): Selection =
        Selection(ids.filterTo(mutableSetOf()) { it == ListenerId || it in present })
}

/**
 * The modifier keys held at the last pointer event over whatever [watch] is put on. A click or
 * a drag reads them as it lands: they come with the pointer event itself, so they are right
 * even when the keys went down while another window had the keyboard.
 */
internal class HeldKeys {
    private var held by mutableStateOf(PointerKeyboardModifiers())

    /** Shift or ⌘: the click adds to the selection. */
    val adding: Boolean get() = held.isShiftPressed || held.isMetaPressed

    /** Alt: the drag leaves copies. */
    val copying: Boolean get() = held.isAltPressed

    val watch: Modifier = Modifier.pointerInput(Unit) {
        awaitPointerEventScope {
            while (true) held = awaitPointerEvent(PointerEventPass.Initial).keyboardModifiers
        }
    }
}

/**
 * What the map and the inspector call the listener when it is selected. Sound ids are lower
 * case, so this one can never be a sound's.
 */
internal const val ListenerId = "Listener"
