package atrium.app.ui

import androidx.compose.ui.input.pointer.PointerIcon

/** A touch screen shows no pointer, so any will do. */
internal actual fun resizePointer(acrossWidth: Boolean): PointerIcon = PointerIcon.Default
