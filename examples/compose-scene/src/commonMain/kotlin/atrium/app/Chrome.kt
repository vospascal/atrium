package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.hoverable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsHoveredAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

/*
 * The editor's chrome: the pieces every panel and list share, so a new panel
 * or list gets them by using them, not by copying them. Nothing here knows
 * about sounds.
 */

/** The pointer for dragging a border left and right, or up and down. Desktop shows a resize arrow. */
internal expect fun resizePointer(acrossWidth: Boolean): PointerIcon

/**
 * A border between two panels that can be dragged to resize them: a hairline
 * with a wider grip, darker while the pointer is over it. [acrossWidth] for a
 * border between side-by-side panels; [onDrag] gets how far it moved.
 */
@Composable
internal fun Splitter(acrossWidth: Boolean, onDrag: (Dp) -> Unit) {
    val drag by rememberUpdatedState(onDrag)
    val density = LocalDensity.current
    val hover = remember { MutableInteractionSource() }
    val hovered by hover.collectIsHoveredAsState()
    val line = if (hovered) Foreground.copy(alpha = .45f) else Hairline
    val grip = if (acrossWidth) Modifier.width(SplitterGrip).fillMaxHeight() else Modifier.height(SplitterGrip).fillMaxWidth()
    Box(
        grip.hoverable(hover)
            .pointerHoverIcon(resizePointer(acrossWidth))
            .pointerInput(acrossWidth) {
                detectDragGestures { change, amount ->
                    change.consume()
                    with(density) { drag((if (acrossWidth) amount.x else amount.y).toDp()) }
                }
            }
            .drawBehind {
                val thickness = if (hovered) 2.dp.toPx() else 1.dp.toPx()
                if (acrossWidth) {
                    drawLine(line, Offset(size.width / 2, 0f), Offset(size.width / 2, size.height), thickness)
                } else {
                    drawLine(line, Offset(0f, size.height / 2), Offset(size.width, size.height / 2), thickness)
                }
            },
    )
}

private val SplitterGrip = 7.dp

/**
 * How a row of a list looks: every other row a shade darker, so a long list
 * keeps its lines; lighter while the pointer is over it; and marked with a bar
 * on its left edge when it is the one selected.
 */
@Composable
internal fun Modifier.listRow(index: Int, selected: Boolean = false): Modifier {
    val hover = remember { MutableInteractionSource() }
    val hovered by hover.collectIsHoveredAsState()
    val zebra = Highlight.copy(alpha = .45f)
    val hoverShade = Highlight
    val chosenShade = Foreground.copy(alpha = .07f)
    val mark = Foreground
    return this
        .hoverable(hover)
        .drawBehind {
            val shade = when {
                selected -> chosenShade
                hovered -> hoverShade
                index % 2 == 1 -> zebra
                else -> Color.Transparent
            }
            drawRect(shade)
            if (selected) drawRect(mark, size = Size(2.dp.toPx(), size.height))
        }
}

/**
 * A column that scrolls, with a thin scrollbar on its right while there is
 * more than fits. The bar can be dragged; the wheel and a trackpad scroll as
 * usual.
 */
@Composable
internal fun ScrollArea(
    modifier: Modifier = Modifier,
    state: ScrollState = rememberScrollState(),
    content: @Composable ColumnScope.() -> Unit,
) {
    Box(modifier) {
        Column(Modifier.fillMaxSize().verticalScroll(state), content = content)
        Scrollbar(state)
    }
}

@Composable
private fun BoxScope.Scrollbar(state: ScrollState) {
    if (state.maxValue <= 0) return
    val scope = rememberCoroutineScope()
    val hover = remember { MutableInteractionSource() }
    val hovered by hover.collectIsHoveredAsState()
    val thumb = Foreground.copy(alpha = if (hovered) .45f else .22f)
    Canvas(
        Modifier.align(Alignment.CenterEnd).width(10.dp).fillMaxHeight()
            .hoverable(hover)
            .pointerInput(state) {
                detectVerticalDragGestures { change, amount ->
                    change.consume()
                    // The thumb moves over the track as the content moves over its whole height.
                    val content = size.height + state.maxValue
                    scope.launch { state.scrollBy(amount * content / size.height) }
                }
            },
    ) {
        val content = size.height + state.maxValue
        val length = (size.height * size.height / content).coerceAtLeast(24.dp.toPx())
        val top = (size.height - length) * state.value / state.maxValue
        val width = if (hovered) 6.dp.toPx() else 4.dp.toPx()
        drawRoundRect(thumb, Offset(size.width - width - 2.dp.toPx(), top), Size(width, length), CornerRadius(width / 2))
    }
}

/** Which panel a toggle in the top bar shows or hides. */
internal enum class PanelSide { Left, Bottom, Right }

/**
 * A small window drawing with one panel filled in, as editors draw their
 * panel toggles: filled while that panel shows, outlined while it is hidden.
 */
@Composable
internal fun PanelToggle(side: PanelSide, shown: Boolean, onClick: () -> Unit) {
    val ink = Foreground
    val hover = remember { MutableInteractionSource() }
    val hovered by hover.collectIsHoveredAsState()
    val hoverShade = Highlight
    Canvas(
        Modifier.size(30.dp)
            .hoverable(hover)
            .drawBehind { if (hovered) drawRoundRect(hoverShade, cornerRadius = CornerRadius(5.dp.toPx())) }
            .clickable(role = Role.Switch, onClick = onClick),
    ) {
        val frame = Size(size.width * .62f, size.height * .5f)
        val origin = Offset((size.width - frame.width) / 2, (size.height - frame.height) / 2)
        val stroke = 1.4.dp.toPx()
        drawRoundRect(ink, origin, frame, CornerRadius(2.dp.toPx()), style = Stroke(stroke))
        val part = when (side) {
            PanelSide.Left -> Pair(origin, Size(frame.width * .32f, frame.height))
            PanelSide.Right -> Pair(Offset(origin.x + frame.width * .68f, origin.y), Size(frame.width * .32f, frame.height))
            PanelSide.Bottom -> Pair(Offset(origin.x, origin.y + frame.height * .62f), Size(frame.width, frame.height * .38f))
        }
        if (shown) {
            drawRoundRect(ink, part.first, part.second, CornerRadius(2.dp.toPx()))
        } else {
            drawRoundRect(ink.copy(alpha = .3f), part.first, part.second, CornerRadius(2.dp.toPx()), style = Stroke(stroke))
        }
    }
}
