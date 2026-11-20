package atrium.app

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.IntSize
import atrium.engine.EmitterKind
import atrium.engine.SceneDescription
import atrium.engine.SceneFile
import atrium.engine.Telemetry
import kotlin.math.atan2
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.sin
import kotlin.math.sqrt

internal enum class PlumeExtent { Point, Fan, Surround }

internal enum class InkBand { Full, Low, High }

internal enum class InkStyle(val layers: List<InkLayer>, val finish: InkFinish = InkFinish.Dye) {
    Original(listOf(InkLayer.Original)),
    Layered(listOf(InkLayer.Veil, InkLayer.Ribbon)),
    Smoke(listOf(InkLayer.SmokeVeil, InkLayer.SmokeFold), InkFinish.Smoke),
    Bloom(listOf(InkLayer.BloomWash, InkLayer.BloomGrain), InkFinish.Bloom);

    val darkBackdrop: Boolean get() = finish != InkFinish.Dye
}

/** The level and frequency balance of a plume from the current audio window. */
internal data class PlumeSignal(val strength: Float, val lowShare: Float, val highShare: Float) {
    /** Bass-heavy to treble-heavy, with midrange and balanced sounds in the middle. */
    val frequencyBalance: Float get() = (highShare - lowShare).coerceIn(-1f, 1f)
}

/**
 * One sound drawn as ink: it flows toward the listener and becomes denser
 * as the sound gets louder.
 *
 * @property direction Screen angle in radians: 0 is right, +π/2 is down.
 * @property strength 0 (silent or muted) to 1 (loud).
 */
internal data class Plume(
    val direction: Float,
    val strength: Float,
    val color: Color,
    val extent: PlumeExtent = PlumeExtent.Point,
    val beamHalfAngle: Float? = null,
    val signal: PlumeSignal? = null,
) {
    fun forBand(band: InkBand): Plume {
        if (strength <= 0f) return this
        val measured = signal ?: return this
        val bandShare = when (band) {
            InkBand.Full -> return copy(strength = measured.strength)
            InkBand.Low -> measured.lowShare
            InkBand.High -> measured.highShare
        }
        val midShare = (1f - measured.lowShare - measured.highShare).coerceAtLeast(0f)
        val share = bandShare + midShare * .5f
        // Keep both layers present, then let the measured bands steer which one dominates.
        val emphasis = (.28f + .82f * sqrt(share.coerceIn(0f, 1f))).coerceAtMost(1f)
        return copy(strength = measured.strength * emphasis)
    }
}

/**
 * When set, every plume is drawn in this one color instead of its own:
 * monochrome ink, black on a light backdrop or white on a dark one.
 */
internal val LocalInkTint = staticCompositionLocalOf<Color?> { null }

/** How dense a plume is before its sound reports a level. */
internal const val RestingStrength = .7f

/** Heard levels (dB SPL) mapped to the faintest and the densest ink. */
private const val QuietLevel = 20f
private const val LoudLevel = 65f
private const val FaintestInk = .06f

/**
 * The plumes for [scene]. Pass [description] and [telemetry] only while that
 * scene plays. Then there is one plume per sound the scene has now, in the
 * order of the description, which differs from the file once a sound has been
 * added or removed; each is where its sound is and as dense as it is heard.
 * Without them each sound of the file rests at its starting direction.
 */
internal fun plumesFor(scene: SceneFile, description: SceneDescription?, telemetry: Telemetry?): List<Plume> {
    if (description == null) {
        return scene.sounds.map { sound ->
            Plume(screenDirection(sound.offsetFromListener.toPoint()), RestingStrength, parseColor(sound.color), PlumeExtent.Point, null, null)
        }
    }
    val listener = description.listener
    return description.sources.map { built ->
        val live = telemetry?.sources?.firstOrNull { it.slot == built.slot }
        val at = live?.position ?: built.position
        val offset = Point(at.x - listener.x, at.y - listener.y)
        val strength = when {
            telemetry != null && live == null -> 0f
            live == null -> RestingStrength
            live.muted -> 0f
            else -> ((built.spl + live.gainDb - QuietLevel) / (LoudLevel - QuietLevel)).coerceIn(FaintestInk, 1f)
        }
        val extent = when {
            built.emitterKind != EmitterKind.FIELD -> PlumeExtent.Point
            offset.hasGroundBearing() -> PlumeExtent.Fan
            else -> PlumeExtent.Surround
        }
        val signal = live?.signal?.let {
            val level = if (it.relativeRms > 0f) {
                built.spl + live.gainDb + 20f * log10(it.relativeRms)
            } else {
                Float.NEGATIVE_INFINITY
            }
            PlumeSignal(
                ((level - QuietLevel) / (LoudLevel - QuietLevel)).coerceIn(0f, 1f),
                it.lowShare,
                it.highShare,
            )
        }
        Plume(screenDirection(offset), strength, parseColor(built.color), extent, built.beamHalfAngle, signal)
    }
}

/** World +x is right and +y is north, which is up on screen. */
internal fun screenDirection(offset: Point): Float =
    if (offset.hasGroundBearing()) atan2(-offset.y, offset.x) else 0f

/** `#rrggbb` → color; anything else → a neutral grey. */
internal fun parseColor(hex: String): Color {
    val digits = hex.removePrefix("#")
    val value = digits.takeIf { it.length == 6 }?.toLongOrNull(16) ?: return Color(0xFF8A9199)
    return Color(0xFF000000 or value)
}

/** Where the listener sits on the live field, as a share of its height. */
private const val ListenerHeight = .42f

/**
 * Ink in water for a canvas of [size] pixels: the simulation, its grid
 * scale, and the bytes it is drawn from. [listener] is the listener's
 * position in pixels, [reach] how far out sources sit, in pixels.
 */
private class Water(
    size: IntSize,
    listener: Offset,
    reach: Float,
    cellsAcross: Int,
    val layer: InkLayer = InkLayer.Original,
    detail: Int = 2,
) {
    val cell = InkFluid.cellSize(size.width.toFloat(), size.height.toFloat(), cellsAcross)
    val fluid = InkFluid(
        columns = ceil(size.width / cell).toInt(),
        rows = ceil(size.height / cell).toInt(),
        listenerX = listener.x / cell,
        listenerY = listener.y / cell,
        reach = reach / cell,
        detail = detail,
        layer = layer,
    )
    val pixels = ByteArray(fluid.dyeColumns * fluid.dyeRows * 4)
}

/**
 * The live ink field: each sound releases simulated pigment on a transparent
 * layer, so the same paint sits on any backdrop.
 * [ring] marks the listener and is the one part that should contrast with
 * the backdrop. A new [flowKey] (another scene) starts from still, clear water.
 */
@Composable
internal fun InkField(
    plumes: List<Plume>,
    ring: Color,
    flowKey: Any,
    modifier: Modifier = Modifier,
    style: InkStyle,
) {
    var canvas by remember { mutableStateOf(IntSize.Zero) }
    val density = LocalDensity.current.density
    val waters = remember(flowKey, canvas, density, style) {
        if (canvas.width == 0 || canvas.height == 0) {
            emptyList()
        } else {
            val listener = Offset(canvas.width / 2f, canvas.height * ListenerHeight)
            val reach = max(canvas.width, canvas.height) * .72f
            val longSideDp = max(canvas.width, canvas.height) / density
            style.layers.map { layer ->
                Water(
                    canvas, listener, reach, cellsAcross = layer.cellsAcross, layer = layer,
                    detail = layer.dyeDetail(longSideDp, layer.cellsAcross),
                )
            }
        }
    }
    val layers = remember(plumes, style) {
        style.layers.associateWith { layer -> plumes.map { it.forBand(layer.band) } }
    }
    val latest by rememberUpdatedState(layers)
    val seconds by produceState(0f, waters) {
        val start = withFrameMillis { it }
        var last = start
        while (true) {
            withFrameMillis { now ->
                val elapsed = ((now - last) / 1000f).coerceAtMost(InkFluid.MaxStep)
                for (water in waters) water.fluid.step(latest[water.layer].orEmpty(), elapsed * water.layer.speed)
                last = now
                value = (now - start) / 1000f
            }
        }
    }
    val tint = LocalInkTint.current
    Canvas(modifier.onSizeChanged { canvas = it }) {
        if (waters.isEmpty()) return@Canvas
        for (water in waters) {
            drawInk(
                water.fluid, water.pixels, water.cell, seconds * water.layer.speed, tint, water.layer.opacity,
                finish = style.finish,
                fine = water.layer.band == InkBand.High,
            )
        }
        drawListener(Offset(size.width / 2, size.height * ListenerHeight), seconds, ring)
    }
}

/**
 * A still of a scene's ink, for covers: its sounds left flowing for a few
 * seconds from clear water. The listener point moves away from where the
 * sounds lean, so a scene whose sounds all lie one way still fills its
 * square.
 */
@Composable
internal fun InkStill(plumes: List<Plume>, modifier: Modifier = Modifier) {
    val tint = LocalInkTint.current
    var canvas by remember { mutableStateOf(IntSize.Zero) }
    val still = remember(plumes, canvas) {
        if (canvas.width == 0 || canvas.height == 0) return@remember null
        val lean = plumes.fold(Offset.Zero) { sum, plume ->
            sum + Offset(cos(plume.direction), sin(plume.direction)) * plume.strength
        } / max(1f, plumes.sumOf { it.strength.toDouble() }.toFloat())
        val middle = Offset(canvas.width / 2f, canvas.height / 2f)
        Water(
            canvas,
            listener = middle - Offset(lean.x * canvas.width, lean.y * canvas.height) * .42f,
            reach = max(canvas.width, canvas.height) * 1.1f,
            cellsAcross = 40,
        ).also { it.fluid.settle(plumes) }
    }
    Canvas(modifier.onSizeChanged { canvas = it }) {
        val current = still ?: return@Canvas
        drawInk(current.fluid, current.pixels, current.cell, 0f, tint)
    }
}

/** Fine rings where the listener is. */
private fun DrawScope.drawListener(center: Offset, seconds: Float, ring: Color) {
    for (index in 0 until 5) {
        val breathe = 1 + .03f * sin(seconds * .8f - index)
        drawCircle(ring, density * (40f + 14f * index) * breathe, center, style = Stroke(width = density * .7f))
    }
}
