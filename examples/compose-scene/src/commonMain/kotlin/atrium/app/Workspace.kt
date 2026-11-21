package atrium.app

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * How the editor's panels are laid out: which show, and how big each is.
 * Kept in [store] and restored at the next launch, the way an editor keeps
 * its workspace. Sizes stay inside sensible limits however far a border is
 * dragged.
 */
@Stable
internal class Workspace(private val store: SettingsStore) {
    var leftShown by remembered("left.shown", true)
    var rightShown by remembered("right.shown", true)
    var bottomShown by remembered("bottom.shown", true)

    var leftWidth by remembered("left.width", 300.dp, SideLimits)
    var rightWidth by remembered("right.width", 320.dp, SideLimits)
    var bottomHeight by remembered("bottom.height", 360.dp, BottomLimits)

    /** Whether any panel shows. */
    val anyShown: Boolean get() = leftShown || rightShown || bottomShown

    /** Hide every panel, or show them all again when none shows: the map alone, as Tab does in a picture editor. */
    fun toggleAll() {
        val show = !anyShown
        leftShown = show
        rightShown = show
        bottomShown = show
    }

    private fun remembered(key: String, default: Boolean) = Stored(
        "$Prefix$key",
        store.read("$Prefix$key")?.toBooleanStrictOrNull() ?: default,
        write = { store.write("$Prefix$key", it.toString()) },
    )

    private fun remembered(key: String, default: Dp, limits: ClosedFloatingPointRange<Float>) = Stored(
        "$Prefix$key",
        (store.read("$Prefix$key")?.toFloatOrNull() ?: default.value).coerceIn(limits).dp,
        write = { store.write("$Prefix$key", it.value.toString()) },
        limit = { it.value.coerceIn(limits).dp },
    )

    private class Stored<T>(val key: String, initial: T, val write: (T) -> Unit, val limit: (T) -> T = { it }) {
        private val state = mutableStateOf(initial)

        operator fun getValue(owner: Any?, property: kotlin.reflect.KProperty<*>): T = state.value

        operator fun setValue(owner: Any?, property: kotlin.reflect.KProperty<*>, value: T) {
            val limited = limit(value)
            if (limited == state.value) return
            state.value = limited
            write(limited)
        }
    }

    companion object {
        private const val Prefix = "editor."
        val SideLimits = 220f..520f
        val BottomLimits = 120f..640f
    }
}
