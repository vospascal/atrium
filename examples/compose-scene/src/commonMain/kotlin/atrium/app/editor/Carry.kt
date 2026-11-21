package atrium.app.editor

import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.unit.IntOffset
import kotlin.math.roundToInt

/**
 * Something picked up in one part of a window and let go over another: a preset from the
 * library dropped on the map, and later anything else. It knows nothing of what it carries or
 * where it goes. A part offers things with [carrySource] and takes them with [carryTarget]; let
 * go anywhere else, the thing is put back.
 *
 * Positions are in the window's root coordinates, where every part can be compared.
 */
@Stable
internal class Carry<T> {
    /** What is being carried, if anything. */
    var item by mutableStateOf<T?>(null)
        private set

    /** Where the pointer carrying it is. */
    var at by mutableStateOf(Offset.Zero)
        private set

    private val targets = mutableMapOf<Any, Target<T>>()

    fun pickUp(item: T, at: Offset) {
        this.item = item
        this.at = at
    }

    fun moveBy(distance: Offset) {
        at += distance
    }

    /** Whether the carried thing is over a part that takes it. */
    val overTarget: Boolean get() = item != null && targets.values.any { it.bounds.contains(at) }

    /** Let go: the part under the pointer gets the thing, at the pointer's place inside it. */
    fun letGo() {
        val carried = item ?: return
        item = null
        val target = targets.values.lastOrNull { it.bounds.contains(at) } ?: return
        target.onDrop(carried, at - target.bounds.topLeft)
    }

    fun putBack() {
        item = null
    }

    fun offer(key: Any, bounds: Rect, onDrop: (T, Offset) -> Unit) {
        targets[key] = Target(bounds, onDrop)
    }

    fun withdraw(key: Any) {
        targets.remove(key)
    }

    private class Target<T>(val bounds: Rect, val onDrop: (T, Offset) -> Unit)
}

/** Lets this part be dragged as [item] by [carry]. */
@Composable
internal fun <T> Modifier.carrySource(carry: Carry<T>, item: T): Modifier {
    val current by rememberUpdatedState(item)
    var placed by remember { mutableStateOf<LayoutCoordinates?>(null) }
    return this
        .onGloballyPositioned { placed = it }
        .pointerInput(carry) {
            detectDragGestures(
                onDragStart = { local -> carry.pickUp(current, (placed?.positionInRoot() ?: Offset.Zero) + local) },
                onDrag = { change, distance ->
                    change.consume()
                    carry.moveBy(distance)
                },
                onDragEnd = carry::letGo,
                onDragCancel = carry::putBack,
            )
        }
}

/** Lets this part take what [carry] brings, with where in it it was let go. */
@Composable
internal fun <T> Modifier.carryTarget(carry: Carry<T>, onDrop: (T, Offset) -> Unit): Modifier {
    val key = remember { Any() }
    val current by rememberUpdatedState(onDrop)
    DisposableEffect(carry) { onDispose { carry.withdraw(key) } }
    return onGloballyPositioned { coordinates ->
        carry.offer(key, coordinates.boundsInRoot()) { item, at -> current(item, at) }
    }
}

/**
 * What is being carried, drawn under the pointer by [label]. Place it over everything the carry
 * can cross, filling the same area.
 */
@Composable
internal fun <T> CarryGhost(carry: Carry<T>, label: @Composable (T, Boolean) -> Unit) {
    var origin by remember { mutableStateOf(Offset.Zero) }
    val item = carry.item
    Box(Modifier.onGloballyPositioned { origin = it.positionInRoot() }) {
        if (item != null) {
            val at = carry.at - origin
            Box(Modifier.offset { IntOffset(at.x.roundToInt() + GhostOffset, at.y.roundToInt() + GhostOffset) }) {
                label(item, carry.overTarget)
            }
        }
    }
}

/** How far right of and below the pointer the carried thing is drawn (px), so the pointer stays visible. */
private const val GhostOffset = 12
