package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.PI
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sin

private val Spectrum = listOf(
    35f to Color(0xFF4168D9),
    42f to Color(0xFF1AAEDB),
    50f to Color(0xFF48D2A0),
    58f to Color(0xFFE8CC64),
    65f to Color(0xFFFF7856),
)

@Composable
internal fun SceneEditor(
    scene: Scene,
    status: String,
    running: Boolean,
    peakDb: Float?,
    onBack: () -> Unit,
    onStepSource: ((Int) -> Unit)?,
    onDrag: (Point) -> Unit,
    onDragFinished: () -> Unit,
) {
    Column(Modifier.fillMaxSize().background(Surface)) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("ATRIUM", Mint, 13, mono = true, strong = true)
            Spacer(Modifier.width(18.dp))
            Text("Composer", Ink, 15, strong = true)
            Spacer(Modifier.weight(1f))
            Box(Modifier.size(7.dp).background(if (running) Mint else Quiet, CircleShape))
            Spacer(Modifier.width(8.dp))
            Text(status)
            Spacer(Modifier.width(12.dp))
            Button("Player", onClick = onBack)
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Edge))
        Row(Modifier.weight(1f)) {
            Box(Modifier.weight(1f).fillMaxHeight()) {
                Viewport(scene, onDrag, onDragFinished, Modifier.fillMaxSize())
                Box(Modifier.align(Alignment.TopStart).padding(18.dp).background(Surface, RoundedCornerShape(5.dp))) {
                    Box(Modifier.padding(horizontal = 9.dp, vertical = 5.dp)) { Text("Perspective") }
                }
                if (!scene.isField && scene.beamHalfAngle == null) {
                    Legend(Modifier.align(Alignment.BottomCenter).padding(18.dp))
                }
            }
            Box(Modifier.width(1.dp).fillMaxHeight().background(Edge))
            Column(Modifier.width(180.dp).fillMaxHeight().padding(16.dp)) {
                Text(
                    when {
                        scene.isField -> "Ambient field"
                        scene.beamHalfAngle != null -> "Directional source"
                        else -> "Omnidirectional source"
                    },
                    size = 12,
                )
                Spacer(Modifier.height(7.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(7.dp).background(Mint, CircleShape))
                    Spacer(Modifier.width(8.dp))
                    Text(scene.emitterName, Ink, 16, strong = true)
                }
                if (onStepSource != null) {
                    Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Button("Prev") { onStepSource(-1) }
                        Button("Next") { onStepSource(1) }
                    }
                }
                Spacer(Modifier.height(18.dp))
                if (scene.isField) {
                    val bearing = scene.offset.takeIf { it.hasGroundBearing() }?.let {
                        (atan2(it.x, it.y) * (180 / PI).toFloat()).roundToInt().mod(360)
                    }
                    Field("From", bearing?.let { "${it}°" } ?: "All directions")
                } else {
                    Field("X", "${decimal(scene.emitter.x)} m")
                    Field("Y", "${decimal(scene.emitter.y)} m")
                    Field("Distance to listener", "${decimal(scene.listener.groundDistanceTo(scene.emitter))} m")
                    scene.beamHalfAngle?.let {
                        Field("Beam half-angle", "${(it * (180 / PI).toFloat()).roundToInt()}°")
                    }
                }
                Spacer(Modifier.height(14.dp))
                Box(Modifier.fillMaxWidth().height(1.dp).background(Edge))
                Spacer(Modifier.height(14.dp))
                Field("Engine peak", peakDb?.let { "${it.roundToInt()} dBFS" } ?: "—")
            }
        }
    }
}

@Composable
private fun Field(label: String, value: String) {
    Column(Modifier.padding(bottom = 12.dp)) {
        Text(label)
        Text(value, Ink, 13, mono = true)
    }
}

@Composable
private fun Legend(modifier: Modifier) {
    val shape = RoundedCornerShape(7.dp)
    Column(modifier.width(240.dp).background(Surface, shape).border(1.dp, Edge, shape).padding(9.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("Estimated direct level", Ink, 11)
            Text("dB SPL", size = 11)
        }
        Spacer(Modifier.height(5.dp))
        Box(
            Modifier.fillMaxWidth().height(5.dp)
                .background(Brush.horizontalGradient(Spectrum.map { it.second }), RoundedCornerShape(2.dp)),
        )
        Spacer(Modifier.height(3.dp))
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            listOf("≤35", "42", "50", "58", "≥65").forEach { Text(it, size = 10, mono = true) }
        }
    }
}

@Composable
private fun Viewport(scene: Scene, move: (Point) -> Unit, finish: () -> Unit, modifier: Modifier) {
    val currentScene = rememberUpdatedState(scene)
    val currentMove = rememberUpdatedState(move)
    val currentFinish = rememberUpdatedState(finish)
    val text = rememberTextMeasurer()
    Canvas(
        modifier.clipToBounds().background(ViewportSurface)
            .semantics {
                contentDescription = if (scene.isField) "Perspective wireframe with an ambient field."
                else "Perspective wireframe. Drag the source marker to move it."
            }
            .pointerInput(scene.width, scene.depth) {
                var dragging: Point? = null
                detectDragGestures(
                    onDragStart = { touch ->
                        val active = currentScene.value
                        if (active.isField) return@detectDragGestures
                        val projection = Projection(Size(size.width.toFloat(), size.height.toFloat()), active.width, active.depth)
                        dragging = active.emitter.takeIf {
                            (projection.project(it) - touch).getDistance() <= 32.dp.toPx()
                        }
                    },
                    onDragEnd = {
                        if (dragging != null) currentFinish.value()
                        dragging = null
                    },
                    onDragCancel = {
                        if (dragging != null) currentFinish.value()
                        dragging = null
                    },
                ) { change, delta ->
                    val point = dragging
                    if (point != null) {
                        val active = currentScene.value
                        val projection = Projection(Size(size.width.toFloat(), size.height.toFloat()), active.width, active.depth)
                        val movement = projection.groundDelta(delta)
                        val next = Point(
                            (point.x + movement.x).coerceIn(0f, active.width),
                            (point.y + movement.y).coerceIn(0f, active.depth),
                            point.z,
                        )
                        dragging = next
                        currentMove.value(next)
                        change.consume()
                    }
                }
            },
    ) {
        val projection = Projection(size, scene.width, scene.depth)
        fun line(from: Point, to: Point, color: Color, width: Float = 1f) {
            drawLine(color, projection.project(from), projection.project(to), strokeWidth = width.dp.toPx())
        }
        val spacing = maxOf(scene.width, scene.depth) / 12f
        for (i in -8..20) {
            val d = i * spacing
            line(Point(d, -8 * spacing), Point(d, 20 * spacing), Edge.copy(alpha = .56f), .7f)
            line(Point(-8 * spacing, d), Point(20 * spacing, d), Edge.copy(alpha = .56f), .7f)
        }
        val corners = listOf(Point(0f, 0f), Point(scene.width, 0f), Point(scene.width, scene.depth), Point(0f, scene.depth))
        corners.forEachIndexed { i, point ->
            val next = corners[(i + 1) % corners.size]
            line(point, next, Color(0xFF4A6172))
            val below = point.copy(z = -spacing / 3)
            line(point, below, Edge)
            line(below, next.copy(z = below.z), Edge)
        }

        if (scene.isField) {
            drawAmbientField(scene, projection)
            val listener = projection.project(scene.listener)
            drawCircle(Ink, 5.dp.toPx(), listener, style = Stroke(1.3.dp.toPx()))
            val label = text.measure("Listener", TextStyle(color = Ink, fontSize = 11.sp))
            drawText(label, topLeft = listener + Offset(9.dp.toPx(), -label.size.height / 2f))
            return@Canvas
        }

        val center = projection.project(scene.emitter.copy(z = 0f))
        val beam = scene.beamHalfAngle
        if (beam != null) {
            drawBeam(scene, projection, center, beam)
        } else {
            fun radius(level: Float) = projection.pixelsPerMetre * scene.referenceDistance *
                10f.pow((scene.levelAtReference - level) / 20f)
            val fade = radius(Spectrum.first().first) * 3.5f
            val stops = buildList {
                add(0f to Spectrum.last().second.copy(alpha = .92f))
                Spectrum.asReversed().forEachIndexed { index, (level, color) ->
                    add(radius(level) / fade to color.copy(alpha = .88f - index * .13f))
                }
                add(.50f to Spectrum.first().second.copy(alpha = .15f))
                add(.73f to Spectrum.first().second.copy(alpha = .045f))
                add(1f to Spectrum.first().second.copy(alpha = 0f))
            }
            scale(1f, Projection.Tilt, pivot = center) {
                drawCircle(Brush.radialGradient(*stops.toTypedArray(), center = center, radius = fade), fade, center)
            }
        }

        val source = projection.project(scene.emitter)
        val listener = projection.project(scene.listener)
        if (beam == null) {
            drawLine(
                Mint.copy(alpha = .78f), source, listener, strokeWidth = 1.3.dp.toPx(),
                pathEffect = PathEffect.dashPathEffect(floatArrayOf(5.dp.toPx(), 4.dp.toPx())),
            )
        }
        fun marker(label: String, point: Offset, color: Color) {
            drawCircle(ViewportSurface, 7.dp.toPx(), point)
            drawCircle(color, 7.dp.toPx(), point, style = Stroke(1.dp.toPx()))
            drawCircle(color, 3.dp.toPx(), point)
            val layout = text.measure(label, TextStyle(color = Ink, fontSize = 11.sp))
            val gap = 6.dp.toPx()
            val width = layout.size.width + 2 * gap
            val height = layout.size.height + gap
            val x = (point.x + 12.dp.toPx()).coerceIn(0f, (size.width - width).coerceAtLeast(0f))
            val y = (point.y - 24.dp.toPx()).coerceAtLeast(0f)
            drawRoundRect(Surface, Offset(x, y), Size(width, height), CornerRadius(4.dp.toPx()))
            drawText(layout, topLeft = Offset(x + gap, y + gap / 2))
        }
        marker("Listener", listener, Ink)
        marker(scene.emitterName, source, Mint)
    }
}

/** Draw a directional guide using the node's facing and beam width. */
private fun DrawScope.drawBeam(scene: Scene, projection: Projection, center: Offset, halfAngle: Float) {
    if (!scene.facing.hasGroundBearing()) return
    val axis = atan2(scene.facing.y, scene.facing.x)
    val reach = minOf(scene.width, scene.depth) * .45f
    fun at(angle: Float) = projection.project(
        Point(scene.emitter.x + cos(angle) * reach, scene.emitter.y + sin(angle) * reach),
    )
    val brush = Brush.radialGradient(
        colors = listOf(Mint.copy(alpha = .84f), Mint.copy(alpha = .32f), Color.Transparent),
        center = center, radius = reach * projection.pixelsPerMetre * Projection.Tilt,
    )
    for (width in listOf(1f, .72f, .43f)) {
        val angle = halfAngle.coerceIn(.035f, PI.toFloat()) * width
        val path = Path().apply {
            moveTo(center.x, center.y)
            for (step in 0..32) {
                val point = at(axis + angle * (step / 16f - 1f))
                lineTo(point.x, point.y)
            }
            close()
        }
        drawPath(path, brush, alpha = .4f)
    }
}

/** Soft bands show a field across the floor. A configured bearing biases the bands toward that edge. */
private fun DrawScope.drawAmbientField(scene: Scene, projection: Projection) {
    val offset = scene.offset
    val directional = offset.hasGroundBearing()
    val headings = if (directional) listOf(atan2(offset.y, offset.x)) else listOf(.36f, 1.94f)
    val extent = minOf(scene.width, scene.depth) * .56f
    for (heading in headings) {
        val dx = cos(heading)
        val dy = sin(heading)
        fun at(along: Float, across: Float): Offset = projection.project(
            Point(scene.width / 2 + dx * along - dy * across, scene.depth / 2 + dy * along + dx * across),
        )
        val brush = Brush.linearGradient(
            0f to Color.Transparent,
            .25f to Mint.copy(alpha = if (directional) .26f else .18f),
            .68f to Mint.copy(alpha = .07f),
            1f to Color.Transparent,
            start = at(extent, 0f),
            end = at(-extent, 0f),
        )
        for (band in -2..2) {
            val across = band * extent * .31f
            val path = Path()
            for (step in 0..32) {
                val t = step / 32f
                val wave = sin(t * 5.8f + band * .4f) * extent * .085f
                val point = at(extent * (1 - 2 * t), across + wave)
                if (step == 0) path.moveTo(point.x, point.y) else path.lineTo(point.x, point.y)
            }
            drawPath(path, brush, alpha = .5f, style = Stroke(18.dp.toPx()))
            drawPath(path, brush, style = Stroke(1.4.dp.toPx()))
        }
    }
}

/** An isometric-style view of a width × depth ground area, fitted to [size]. */
internal class Projection(size: Size, width: Float, depth: Float) {
    private val diagonals = (width + depth) * Diagonal
    val pixelsPerMetre = (minOf(size.width * .84f, size.height * 1.36f) / diagonals).coerceAtLeast(.01f)
    private val unit = pixelsPerMetre * Diagonal
    private val origin = Offset(
        size.width / 2 - (width - depth) / 2 * unit,
        size.height * .48f - (width + depth) / 2 * unit * Tilt,
    )

    fun project(point: Point) = origin + Offset(
        (point.x - point.y) * unit,
        (point.x + point.y) * unit * Tilt - point.z * pixelsPerMetre,
    )

    fun groundDelta(delta: Offset): Offset {
        val across = delta.x / unit
        val down = delta.y / (unit * Tilt)
        return Offset((across + down) / 2, (down - across) / 2)
    }

    companion object {
        private const val Diagonal = .70710677f
        const val Tilt = .52f
    }
}

private fun decimal(value: Float): String {
    val tenths = (value * 10).roundToInt()
    val fraction = kotlin.math.abs(tenths % 10)
    return "${if (tenths < 0) "-" else ""}${kotlin.math.abs(tenths / 10)}.$fraction"
}
