package atrium.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.runtime.produceState
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.sin
import atrium.app.SettingsStore

/**
 * The player's surroundings for the ink: light paper, or near-black in dark
 * mode. The ink itself is the same in both; only what frames it changes.
 */
@Immutable
internal data class PlayerColors(
    val backdrop: Color,
    val foreground: Color,
    val monochromeInk: Color,
    val muted: Color,
    val hairline: Color,
    val highlight: Color,
    val ring: Color,
    /** The chosen half of a two-way toggle, and its label. */
    val chosen: Color,
    val onChosen: Color,
    /** The other half: filled in light mode, open in dark mode. */
    val other: Color,
    val onOther: Color,
)

internal val LightPlayer = PlayerColors(
    backdrop = Color(0xFFFCFBF9),
    foreground = Color(0xFF151515),
    monochromeInk = Color.Black,
    muted = Color(0xFF7A7C7F),
    hairline = Color(0xFFE4E2DF),
    highlight = Color(0xFFF0EFED),
    ring = Color(0xFF6B7075).copy(alpha = .3f),
    chosen = Color(0xFFFCFBF9),
    onChosen = Color(0xFF151515),
    other = Color(0xFF151515),
    onOther = Color(0xFFFCFBF9),
)

internal val DarkPlayer = PlayerColors(
    backdrop = Color(0xFF0E0F10),
    foreground = Color(0xFFEDEBE7),
    monochromeInk = Color.White,
    muted = Color(0xFF9A9C9F),
    hairline = Color(0xFF2A2C2E),
    highlight = Color(0xFF1B1D1F),
    ring = Color(0xFFE8E8E8).copy(alpha = .5f),
    chosen = Color(0xFFF4F3F1),
    onChosen = Color(0xFF0E0F10),
    other = Color.Transparent,
    onOther = Color(0xFFEDEBE7),
)

internal val LocalPlayerColors = staticCompositionLocalOf { LightPlayer }

/** Light, dark, or whichever the system uses. */
internal enum class Appearance { Auto, Light, Dark }

/**
 * How the player looks. Restored from [store] when created and written back
 * on every change, so it outlives both a visit to the editor and a restart.
 */
@Stable
internal class Look(private val store: SettingsStore) {
    private val appearanceState = mutableStateOf(
        store.read(AppearanceKey)?.let { saved -> Appearance.entries.firstOrNull { it.name == saved } } ?: Appearance.Auto,
    )
    private val monochromeState = mutableStateOf(store.read(MonochromeKey) == "true")

    var appearance: Appearance
        get() = appearanceState.value
        set(value) {
            appearanceState.value = value
            store.write(AppearanceKey, value.name)
        }

    var monochrome: Boolean
        get() = monochromeState.value
        set(value) {
            monochromeState.value = value
            store.write(MonochromeKey, value.toString())
        }

    private companion object {
        const val AppearanceKey = "appearance"
        const val MonochromeKey = "ink.monochrome"
    }
}

/** The player's colors for [appearance]. Auto follows the system. */
@Composable
internal fun PlayerTheme(appearance: Appearance, content: @Composable () -> Unit) {
    val dark = when (appearance) {
        Appearance.Auto -> isSystemInDarkTheme()
        Appearance.Light -> false
        Appearance.Dark -> true
    }
    val colors = if (dark) DarkPlayer else LightPlayer
    CompositionLocalProvider(LocalPlayerColors provides colors, content = content)
}

internal val Backdrop: Color @Composable get() = LocalPlayerColors.current.backdrop
internal val Foreground: Color @Composable get() = LocalPlayerColors.current.foreground
internal val Muted: Color @Composable get() = LocalPlayerColors.current.muted
internal val Hairline: Color @Composable get() = LocalPlayerColors.current.hairline
internal val Highlight: Color @Composable get() = LocalPlayerColors.current.highlight

@Composable
internal fun PaperText(
    value: String,
    size: Int,
    color: Color = Foreground,
    weight: FontWeight = FontWeight.Normal,
    spacing: Float = 0f,
    modifier: Modifier = Modifier,
    align: TextAlign = TextAlign.Unspecified,
    /** Kept to one line, cut short with an ellipsis where it does not fit. */
    singleLine: Boolean = false,
) {
    BasicText(
        value,
        modifier,
        style = TextStyle(color = color, fontSize = size.sp, fontWeight = weight, letterSpacing = spacing.sp, textAlign = align),
        maxLines = if (singleLine) 1 else Int.MAX_VALUE,
        softWrap = !singleLine,
        overflow = if (singleLine) androidx.compose.ui.text.style.TextOverflow.Ellipsis else androidx.compose.ui.text.style.TextOverflow.Clip,
    )
}

@Composable
internal fun Wordmark() = PaperText("ATRIUM", 20, weight = FontWeight.Bold, spacing = 3.5f)

/** A choice chip: filled when [selected]. */
@Composable
internal fun Chip(label: String, selected: Boolean, onClick: () -> Unit) {
    val shape = RoundedCornerShape(50)
    Box(
        Modifier.border(1.dp, if (selected) Foreground else Hairline, shape)
            .background(if (selected) Foreground else Backdrop, shape)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 8.dp),
    ) {
        PaperText(label, 14, if (selected) Backdrop else Foreground)
    }
}

/**
 * A thin 0–1 level on paper: tap or drag to set it. Once pressed it takes the
 * arrow keys, moving by [step] a press and ten times that with Shift.
 */
@Composable
internal fun PaperLevel(value: Float, onChange: (Float) -> Unit, modifier: Modifier = Modifier, step: Float = .01f) {
    val change = rememberUpdatedState(onChange)
    val now = rememberUpdatedState(value)
    val focus = remember { FocusRequester() }
    val track = Hairline
    val fill = Foreground
    Box(
        modifier.height(28.dp)
            .keyboardSlider(focus) { steps -> change.value((now.value + steps * step).coerceIn(0f, 1f)) }
            .pointerInput(Unit) {
                detectTapGestures {
                    focus.requestFocus()
                    change.value((it.x / size.width).coerceIn(0f, 1f))
                }
            }
            .pointerInput(Unit) {
                detectDragGestures(onDragStart = { focus.requestFocus() }) { pointer, _ ->
                    change.value((pointer.position.x / size.width).coerceIn(0f, 1f))
                }
            },
    ) {
        Canvas(Modifier.fillMaxWidth().fillMaxHeight()) {
            val middle = size.height / 2
            val filled = size.width * value.coerceIn(0f, 1f)
            drawLine(track, Offset(0f, middle), Offset(size.width, middle), strokeWidth = 2.dp.toPx())
            drawLine(fill, Offset(0f, middle), Offset(filled, middle), strokeWidth = 2.dp.toPx())
            drawCircle(fill, 7.dp.toPx(), Offset(filled, middle))
        }
    }
}

@Composable
internal fun PlayIcon(color: Color, size: Dp, outline: Boolean = false) {
    Canvas(Modifier.size(size)) {
        val path = Path().apply {
            moveTo(this@Canvas.size.width * .24f, this@Canvas.size.height * .16f)
            lineTo(this@Canvas.size.width * .84f, this@Canvas.size.height * .5f)
            lineTo(this@Canvas.size.width * .24f, this@Canvas.size.height * .84f)
            close()
        }
        if (outline) drawPath(path, color, style = Stroke(width = 2.dp.toPx())) else drawPath(path, color)
    }
}

@Composable
internal fun StopIcon(color: Color, size: Dp) {
    Canvas(Modifier.size(size)) {
        drawRoundRect(color, cornerRadius = CornerRadius(this.size.width * .12f))
    }
}

@Composable
internal fun PauseIcon(color: Color, size: Dp) {
    Canvas(Modifier.size(size)) {
        val bar = Size(this.size.width * .2f, this.size.height * .64f)
        val top = this.size.height * .18f
        val radius = CornerRadius(bar.width / 3)
        drawRoundRect(color, Offset(this.size.width * .22f, top), bar, radius)
        drawRoundRect(color, Offset(this.size.width * .58f, top), bar, radius)
    }
}

/** Level bars that move while [playing]; still otherwise. */
@Composable
internal fun LevelBarsIcon(playing: Boolean, size: Dp) {
    val seconds by produceState(0f, playing) {
        if (!playing) return@produceState
        val start = withFrameMillis { it }
        while (true) withFrameMillis { value = (it - start) / 1000f }
    }
    val color = Foreground
    Canvas(Modifier.size(size)) {
        val bars = 4
        val gap = this.size.width / (bars * 2f)
        repeat(bars) { bar ->
            val wave = if (playing) .5f + .5f * sin(seconds * (3.1f + bar * .9f) + bar * 1.7f) else .5f
            val height = this.size.height * (.3f + .65f * wave)
            val x = gap * (bar * 2 + .5f)
            drawLine(
                color,
                Offset(x, this.size.height),
                Offset(x, this.size.height - height),
                strokeWidth = gap * .9f,
            )
        }
    }
}
