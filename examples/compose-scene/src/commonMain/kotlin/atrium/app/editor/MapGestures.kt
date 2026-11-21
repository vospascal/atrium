package atrium.app.editor

import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.pointerInput
import atrium.app.Point
import kotlin.math.pow

/** The sound a pointer at [at] picks in [view] among [markers], if any. */
internal fun pickMarker(markers: List<Marker>, view: TopDown, at: Offset, density: Float): Marker? {
    val (washes, points) = markers.withIndex().partition { it.value.ambient }
    val index = pickAt(
        points.map { it.index to view.toScreen(it.value.offset) },
        washes.map { it.index to view.toScreen(it.value.offset) },
        AmbientRadius * view.pixelsPerMetre,
        at,
        PickRadius * density,
    )
    return index?.let { markers[it] }
}

/** What a click at [at] in [view] lands on: the listener at [listenerAt], a sound's id, or null for the ground. */
internal fun clickedAt(view: TopDown, at: Offset, density: Float, markers: List<Marker>, listenerAt: Point): String? {
    val onListener = (view.toScreen(listenerAt) - at).getDistance() <= PickRadius * density
    // The listener lies on top: a sound over it, or an ambient one anchored at it, never hides it.
    return if (onListener) ListenerId else pickMarker(markers, view, at, density)?.id
}

/** What a drag takes hold of where it starts. */
internal sealed interface Grab {
    /** The facing handle of the selected sound: the drag turns it. */
    data class Facing(val marker: Marker) : Grab

    /** The facing handle of the selected listener: the drag turns the listener. */
    data object ListenerFacing : Grab

    /** The listener: the drag moves it. */
    data object Listener : Grab

    /** A sound: the drag moves it, with every other selected sound. */
    data class Sound(val marker: Marker) : Grab

    /** Empty ground: the drag moves the view. */
    data object Ground : Grab
}

/**
 * What a drag that starts at [at] in [view] takes hold of, with [selection] selected and the
 * listener at [listenerAt] facing [listenerFacing].
 */
internal fun grabAt(
    view: TopDown,
    at: Offset,
    density: Float,
    markers: List<Marker>,
    selection: Selection,
    listenerAt: Point,
    listenerFacing: Float,
): Grab {
    // The facing handle of the selected sound is tried first: it sits away from its dot.
    val focused = markers.firstOrNull { it.id == selection.single && it.turnable }
    val handle = focused?.let { facingHandle(view.toScreen(it.offset), it.facing, FacingReach * density) }
    if (focused != null && handle != null && (handle - at).getDistance() <= HandleRadius * density) {
        return Grab.Facing(focused)
    }
    val listenerOnScreen = view.toScreen(listenerAt)
    if (selection.single == ListenerId &&
        (facingHandle(listenerOnScreen, listenerFacing, FacingReach * density) - at).getDistance() <= HandleRadius * density
    ) {
        return Grab.ListenerFacing
    }
    // The listener before any sound, as it is drawn: on top.
    if ((listenerOnScreen - at).getDistance() <= PickRadius * density) return Grab.Listener
    return pickMarker(markers, view, at, density)?.let { Grab.Sound(it) } ?: Grab.Ground
}

/** The wheel or two fingers zoom about the pointer, as on a map: [onZoom] gets the factor and the point to zoom about. */
internal fun Modifier.zoomOnScroll(key1: Any?, key2: Any?, onZoom: (factor: Float, about: Offset) -> Unit): Modifier =
    pointerInput(key1, key2) {
        awaitPointerEventScope {
            while (true) {
                val event = awaitPointerEvent()
                if (event.type != PointerEventType.Scroll) continue
                val change = event.changes.firstOrNull() ?: continue
                onZoom(ZoomPerNotch.pow(-change.scrollDelta.y), change.position)
                change.consume()
            }
        }
    }

/** How much one notch of a mouse wheel zooms. */
private const val ZoomPerNotch = 1.12f

/** How close to a sound a click or the start of a drag has to be (dp). */
private const val PickRadius = 20f

/** How close to a facing handle a drag has to start to grab it (dp). */
private const val HandleRadius = 16f

/** How near its anchor an ambient sound can be grabbed (m). */
private const val AmbientRadius = 6f
