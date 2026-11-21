package atrium.app

import androidx.compose.ui.input.pointer.PointerIcon
import java.awt.Cursor

internal actual fun resizePointer(acrossWidth: Boolean): PointerIcon =
    PointerIcon(Cursor(if (acrossWidth) Cursor.E_RESIZE_CURSOR else Cursor.N_RESIZE_CURSOR))
