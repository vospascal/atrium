package atrium.app

// The composer's dark theme. The player has its own, in Paper.kt.

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

internal val Surface = Color(0xFF151B22)
internal val ViewportSurface = Color(0xFF0D131B)
internal val Edge = Color(0xFF2B3947)
internal val Ink = Color(0xFFEAF2F7)
internal val Quiet = Color(0xFFA1B3C1)
internal val Mint = Color(0xFF83E7D4)

@Composable
internal fun Text(
    value: String,
    color: Color = Quiet,
    size: Int = 12,
    mono: Boolean = false,
    strong: Boolean = false,
) {
    BasicText(
        value,
        style = TextStyle(
            color = color,
            fontSize = size.sp,
            fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default,
            fontWeight = if (strong) FontWeight.SemiBold else FontWeight.Normal,
        ),
    )
}

/** A bordered text button; [selected] fills it. */
@Composable
internal fun Button(label: String, selected: Boolean = false, onClick: () -> Unit) {
    val shape = RoundedCornerShape(6.dp)
    Box(
        Modifier.border(1.dp, if (selected) Mint else Edge, shape)
            .background(if (selected) Mint.copy(alpha = .14f) else Surface, shape)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 7.dp),
    ) {
        Text(label, if (selected) Mint else Ink)
    }
}
