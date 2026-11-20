package atrium.app

import androidx.compose.ui.graphics.Color
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/** The most sounds released into the water: the engine's source pool. */
internal const val MaxPlumes = 16

internal enum class InkLayer(
    val cellsAcross: Int,
    val width: Float,
    val speed: Float,
    val opacity: Float,
    val turn: Float,
    private val maxDyeDetail: Int,
    private val dpPerDyeCell: Float,
    val sway: Float = 0f,
    val confinement: Float = 1f,
    val core: Float = 1f,
    val band: InkBand = InkBand.Full,
) {
    Original(112, 1f, 1f, 1f, 0f, 2, 7f),
    Veil(68, 1.55f, .80f, .42f, -.22f, 3, 7f, band = InkBand.Low),
    Ribbon(104, 1.08f, 1f, .82f, .15f, 6, 3f, band = InkBand.High),
    SmokeVeil(68, 1.52f, .78f, .56f, -.24f, 3, 7f, sway = .26f, confinement = 1.35f, band = InkBand.Low),
    SmokeFold(104, 1.32f, 1.06f, .96f, .20f, 6, 3f, sway = .48f, confinement = 1.85f, band = InkBand.High),
    BloomWash(68, 2.08f, .73f, .87f, -.28f, 3, 7f, sway = .37f, confinement = 1.45f, core = 1.63f, band = InkBand.Low),
    BloomGrain(104, 1.60f, 1.13f, .88f, .14f, 6, 3f, sway = .58f, confinement = 1.76f, core = 1.38f, band = InkBand.High);

    fun dyeDetail(longSideDp: Float, waterCells: Int): Int =
        if (longSideDp <= 900f) 2 else {
            ceil(longSideDp / (waterCells * dpPerDyeCell)).toInt().coerceIn(2, maxDyeDetail)
        }
}

private val FanAngles = floatArrayOf(-.66f, -.33f, 0f, .33f, .66f)
private const val SpringsPerPlume = 8
private val SurroundAngles = FloatArray(SpringsPerPlume) { it * (2 * PI / SpringsPerPlume).toFloat() }

/**
 * Ink in water, simulated: incompressible Navier–Stokes after Jos Stam's
 * "Stable Fluids" (semi-Lagrangian advection, pressure projection), with
 * vorticity confinement (Fedkiw, Stam & Jensen 2001).
 *
 * Each sounding source is a spring at the edge of the water, where its
 * direction leaves the listener: it releases ink in its color and pushes
 * water inward. Zen balances those springs at the listener. The layered modes
 * balance them at the perimeter, and their layers steer in different directions
 * so the listener sits within the flow. Louder sources release more ink and
 * push harder, and moving sources stir eddies. When a source falls silent its
 * spring stops, the water comes to rest, and the remaining ink fades.
 *
 * Ink barely diffuses in water: its edges stay sharp and are stretched into
 * thin sheets and filaments. So the dye lives on a grid [detail] times finer
 * than the water, and is carried with MacCormack advection (a backward step,
 * a forward check, and half the error corrected, clamped so it cannot
 * overshoot), which keeps edges that plain advection would blur into smoke.
 *
 * Units are water cells and seconds. The water grid is [columns] × [rows];
 * cell (x, y) spans x..x+1, y..y+1. The listener sits at ([listenerX],
 * [listenerY]) and sources at most [reach] cells away.
 */
internal class InkFluid(
    val columns: Int,
    val rows: Int,
    val listenerX: Float = columns / 2f,
    val listenerY: Float = rows / 2f,
    val reach: Float = max(columns, rows) * .72f,
    val detail: Int = 2,
    private val layer: InkLayer = InkLayer.Original,
) {
    private val cells = columns * rows
    private val throughFlow = layer != InkLayer.Original
    private val outletRadius = max(DrainRadius, min(columns, rows) * .18f)

    private var velocityX = FloatArray(cells)
    private var velocityY = FloatArray(cells)
    private var nextX = FloatArray(cells)
    private var nextY = FloatArray(cells)
    private var pressure = FloatArray(cells)
    private var pressureNext = FloatArray(cells)
    private val divergence = FloatArray(cells)
    private val wanted = FloatArray(cells)
    private val curl = FloatArray(cells)

    /** The dye grid. */
    val dyeColumns = columns * detail
    val dyeRows = rows * detail
    private val dyeCells = dyeColumns * dyeRows

    // Pigment: amount, and color premultiplied by amount, so mixed inks average.
    private var amount = FloatArray(dyeCells)
    private var red = FloatArray(dyeCells)
    private var green = FloatArray(dyeCells)
    private var blue = FloatArray(dyeCells)
    private var predicted = FloatArray(dyeCells)
    private var corrected = FloatArray(dyeCells)

    // Where each dye cell's ink came from (back) and would go (forward), as
    // a top-left dye index and the fractions across and down.
    private val backAt = IntArray(dyeCells)
    private val backAcross = FloatArray(dyeCells)
    private val backDown = FloatArray(dyeCells)
    private val forwardAt = IntArray(dyeCells)
    private val forwardAcross = FloatArray(dyeCells)
    private val forwardDown = FloatArray(dyeCells)

    /** Seconds of water so far: drives the uneven release of ink. */
    private var clock = 0f

    /** 0 to 1: how hard the sources are working, eased; confinement follows it, so still water calms. */
    private var activity = 0f

    private val lastSpringX = FloatArray(MaxPlumes * SpringsPerPlume) { Float.NaN }
    private val lastSpringY = FloatArray(lastSpringX.size)
    private val easedDragX = FloatArray(lastSpringX.size)
    private val easedDragY = FloatArray(lastSpringX.size)
    private val lastExtent = arrayOfNulls<PlumeExtent>(MaxPlumes)
    private val easedBearing = FloatArray(MaxPlumes)

    /** Advance the water by [elapsed] seconds with [plumes] as they are now. */
    fun step(plumes: List<Plume>, elapsed: Float) {
        val dt = elapsed.coerceIn(0f, MaxStep)
        if (dt <= 0f) return
        clock += dt
        wanted.fill(0f)
        var released = 0f
        var surroundWater = 0f
        for (slot in 0 until MaxPlumes) {
            val plume = plumes.getOrNull(slot)
            val extent = when {
                plume == null || plume.strength <= 0f -> null
                !throughFlow -> PlumeExtent.Point
                else -> plume.extent
            }
            val firstSpring = slot * SpringsPerPlume
            if (extent != lastExtent[slot]) {
                lastSpringX.fill(Float.NaN, firstSpring, firstSpring + SpringsPerPlume)
                easedDragX.fill(0f, firstSpring, firstSpring + SpringsPerPlume)
                easedDragY.fill(0f, firstSpring, firstSpring + SpringsPerPlume)
                lastExtent[slot] = extent
                if (plume != null) easedBearing[slot] = plume.direction
            }
            if (plume == null || extent == null) continue
            val bearing = if (!throughFlow) {
                plume.direction
            } else {
                // Follow the shorter arc when motion crosses the angle seam.
                val delta = plume.direction - easedBearing[slot]
                easedBearing[slot] += atan2(sin(delta), cos(delta)) * min(1f, dt * 11f)
                easedBearing[slot]
            }
            val angles = when (extent) {
                PlumeExtent.Point -> null
                PlumeExtent.Fan -> FanAngles
                PlumeExtent.Surround -> SurroundAngles
            }
            val parts = angles?.size ?: 1
            var plumeWater = 0f
            for (part in 0 until parts) {
                val angle = bearing + (angles?.get(part) ?: 0f)
                val heading = when (extent) {
                    PlumeExtent.Point -> angle + layer.sway * sin(clock * .92f + slot * 1.5f)
                    PlumeExtent.Fan -> bearing
                    PlumeExtent.Surround -> {
                        // Let unplaced winds skim in from the edges in broad
                        // currents. The layers curl in opposite directions.
                        val drift = if (layer.turn < 0f) -.72f else .72f
                        angle + drift + .2f * sin(clock * .45f + part * .8f)
                    }
                }
                plumeWater += spring(firstSpring + part, slot + part * .41f, plume, angle, heading + layer.turn, 1f / parts, dt)
            }
            when {
                !throughFlow -> released += plumeWater
                extent == PlumeExtent.Surround -> surroundWater += plumeWater
                else -> {
                    val outletBearing = bearing + layer.turn
                    val (x, y) = springFor(-cos(outletBearing), -sin(outletBearing))
                    splatWanted(x, y, outletRadius, -plumeWater)
                }
            }
        }
        if (released > 0f) splatWanted(listenerX, listenerY, DrainRadius, -released)
        if (surroundWater > 0f) {
            // Four broad outlets keep the surround field balanced in every direction.
            val share = -surroundWater / 4f
            splatWanted(Inset, rows / 2f, outletRadius, share)
            splatWanted(columns - Inset, rows / 2f, outletRadius, share)
            splatWanted(columns / 2f, Inset, outletRadius, share)
            splatWanted(columns / 2f, rows - Inset, outletRadius, share)
        }
        val working = plumes.take(MaxPlumes).maxOfOrNull { it.strength.coerceIn(0f, 1f) } ?: 0f
        activity += (working - activity) * min(1f, dt * .8f)

        confineVorticity(dt)
        slowDown(dt)
        advectVelocity(dt)
        project()

        traceDye(dt)
        amount = carried(amount)
        red = carried(red)
        green = carried(green)
        blue = carried(blue)
        fadeDye(dt)
    }

    /** One source's spring: ink, inward push, and the drag of its motion. Returns the water it adds. */
    private fun spring(
        index: Int, phase: Float, plume: Plume, angle: Float, heading: Float, share: Float, dt: Float,
    ): Float {
        val directionX = cos(angle)
        val directionY = sin(angle)
        val (springX, springY) = springFor(directionX, directionY)
        val strength = plume.strength.coerceIn(0f, 1f)
        val field = share < 1f
        val surround = field && plume.extent == PlumeExtent.Surround
        val beam = plume.beamHalfAngle
        val width = when {
            field -> 1.45f
            throughFlow && beam != null -> .46f + .42f * (beam / (PI / 2).toFloat()).coerceIn(0f, 1f)
            else -> 1f
        }
        // Zen has one layer, so frequency balance shapes the same plume instead of splitting it.
        val frequencyBalance = if (layer.band == InkBand.Full) plume.signal?.frequencyBalance ?: 0f else 0f
        val radius = SpringRadius * (.6f + .6f * strength) * layer.width * width * (1f - .26f * frequencyBalance)
        val jet = JetSpeed * (.4f + .6f * strength) * (1f + .12f * frequencyBalance) * when {
            surround -> .56f
            field -> .72f
            else -> 1f
        }
        val pull = min(1f, dt * 8f)

        // A moving source drags the water along: the only stirring there is.
        var dragX = 0f
        var dragY = 0f
        if (!lastSpringX[index].isNaN()) {
            dragX = ((springX - lastSpringX[index]) / dt).coerceIn(-MaxDrag, MaxDrag)
            dragY = ((springY - lastSpringY[index]) / dt).coerceIn(-MaxDrag, MaxDrag)
        }
        lastSpringX[index] = springX
        lastSpringY[index] = springY
        if (throughFlow) {
            // Positions arrive in discrete telemetry frames. Keep momentum
            // between updates so a moving source leaves a continuous wake.
            val follow = min(1f, dt * 5.5f)
            easedDragX[index] += (dragX - easedDragX[index]) * follow
            easedDragY[index] += (dragY - easedDragY[index]) * follow
            dragX = easedDragX[index]
            dragY = easedDragY[index]
        }
        val inwardX = -cos(heading) * jet
        val inwardY = -sin(heading) * jet
        splat(springX, springY, radius) { cell, weight ->
            val toward = weight * pull
            velocityX[cell] += (inwardX + dragX * DragShare - velocityX[cell]) * toward
            velocityY[cell] += (inwardY + dragY * DragShare - velocityY[cell]) * toward
        }

        // Ink leaves in uneven puffs from a tight core with a long thin tail,
        // as ink does entering water; the denser bands drifting inward are
        // what make the flow visible.
        val supply = if (field) {
            // A shared, slow gust makes neighboring inlets read as one wash.
            .44f + .14f * sin(clock * .92f + phase * .48f)
        } else {
            val puff = .5f + .5f * sin(clock * 1.9f + phase * 2.3f) * sin(clock * .73f + phase * 1.1f)
            .15f + .85f * puff * puff
        }
        val ink = InkRate * strength * share * supply * dt
        val color = plume.color
        val core = DyeCore * layer.core * when {
            surround -> 3.4f
            field -> 2.8f
            else -> 1f
        }
        releaseDye(springX, springY, radius * 1.4f, core) { cell, weight ->
            amount[cell] += ink * weight
            red[cell] += color.red * ink * weight
            green[cell] += color.green * ink * weight
            blue[cell] += color.blue * ink * weight
        }
        val water = SpringFlow * strength * share
        splatWanted(springX, springY, radius, water)
        return water
    }

    /** Where a source in [directionX], [directionY] meets the water: [reach] out, or the nearest edge. */
    private fun springFor(directionX: Float, directionY: Float): Pair<Float, Float> {
        var distance = reach
        if (directionX > 1e-4f) distance = min(distance, (columns - Inset - listenerX) / directionX)
        if (directionX < -1e-4f) distance = min(distance, (Inset - listenerX) / directionX)
        if (directionY > 1e-4f) distance = min(distance, (rows - Inset - listenerY) / directionY)
        if (directionY < -1e-4f) distance = min(distance, (Inset - listenerY) / directionY)
        return (listenerX + directionX * distance) to (listenerY + directionY * distance)
    }

    /** Calls [apply] for each water cell near ([centerX], [centerY]) with a Gaussian weight. */
    private inline fun splat(centerX: Float, centerY: Float, radius: Float, apply: (cell: Int, weight: Float) -> Unit) {
        val span = (radius * 2.2f).toInt() + 1
        val r2 = radius * radius
        val fromX = max(0, (centerX - span).toInt())
        val toX = min(columns - 1, (centerX + span).toInt())
        val fromY = max(0, (centerY - span).toInt())
        val toY = min(rows - 1, (centerY + span).toInt())
        for (y in fromY..toY) for (x in fromX..toX) {
            val dx = x + .5f - centerX
            val dy = y + .5f - centerY
            val weight = exp(-(dx * dx + dy * dy) / r2)
            if (weight > .01f) apply(y * columns + x, weight)
        }
    }

    /**
     * Calls [apply] for each dye cell within [radius] water cells of
     * ([centerX], [centerY]), weighted by a sharp core and a 1/r^1.75 tail.
     */
    private inline fun releaseDye(
        centerX: Float, centerY: Float, radius: Float, core: Float, apply: (cell: Int, weight: Float) -> Unit,
    ) {
        val fromX = max(0, ((centerX - radius) * detail).toInt())
        val toX = min(dyeColumns - 1, ((centerX + radius) * detail).toInt())
        val fromY = max(0, ((centerY - radius) * detail).toInt())
        val toY = min(dyeRows - 1, ((centerY + radius) * detail).toInt())
        val core2 = core * core
        val r2 = radius * radius
        for (y in fromY..toY) for (x in fromX..toX) {
            val dx = (x + .5f) / detail - centerX
            val dy = (y + .5f) / detail - centerY
            val d2 = dx * dx + dy * dy
            if (d2 > r2) continue
            val weight = (core2 / (d2 + core2)).pow(.875f) * (1f - d2 / r2)
            apply(y * dyeColumns + x, weight)
        }
    }

    /** Adds [total] wanted divergence around a point, spread so it sums to [total]. */
    private fun splatWanted(centerX: Float, centerY: Float, radius: Float, total: Float) {
        var sum = 0f
        splat(centerX, centerY, radius) { _, weight -> sum += weight }
        if (sum <= 0f) return
        splat(centerX, centerY, radius) { cell, weight -> wanted[cell] += total * weight / sum }
    }

    /** Keeps small swirls spinning, which grids otherwise smear away: low viscosity, billowing ink. */
    private fun confineVorticity(dt: Float) {
        for (y in 1 until rows - 1) for (x in 1 until columns - 1) {
            val cell = y * columns + x
            curl[cell] = .5f * ((velocityY[cell + 1] - velocityY[cell - 1]) - (velocityX[cell + columns] - velocityX[cell - columns]))
        }
        val strength = Confinement * layer.confinement * activity * dt
        if (strength <= 0f) return
        for (y in 2 until rows - 2) for (x in 2 until columns - 2) {
            val cell = y * columns + x
            val gradientX = .5f * (abs(curl[cell + 1]) - abs(curl[cell - 1]))
            val gradientY = .5f * (abs(curl[cell + columns]) - abs(curl[cell - columns]))
            val length = sqrt(gradientX * gradientX + gradientY * gradientY) + 1e-5f
            val spin = curl[cell]
            velocityX[cell] += strength * (gradientY / length) * spin
            velocityY[cell] -= strength * (gradientX / length) * spin
        }
    }

    /** Damping plus a small constant drag, so water that is no longer pushed truly stops. */
    private fun slowDown(dt: Float) {
        val damping = exp(-Damping * dt)
        val drag = LinearDrag * dt
        for (cell in 0 until cells) {
            val vx = velocityX[cell] * damping
            val vy = velocityY[cell] * damping
            val speed = sqrt(vx * vx + vy * vy)
            val keep = if (speed > drag) (speed - drag) / speed else 0f
            velocityX[cell] = vx * keep
            velocityY[cell] = vy * keep
        }
    }

    /** Semi-Lagrangian: each water cell takes the velocity upstream of it. */
    private fun advectVelocity(dt: Float) {
        for (y in 0 until rows) for (x in 0 until columns) {
            val cell = y * columns + x
            val fromX = (x - dt * velocityX[cell]).coerceIn(0f, columns - 1.001f)
            val fromY = (y - dt * velocityY[cell]).coerceIn(0f, rows - 1.001f)
            nextX[cell] = sampleWater(velocityX, fromX, fromY)
            nextY[cell] = sampleWater(velocityY, fromX, fromY)
        }
        var previous = velocityX
        velocityX = nextX
        nextX = previous
        previous = velocityY
        velocityY = nextY
        nextY = previous
    }

    /** Bilinear sample of a water field at index coordinates (cell centres at whole numbers). */
    private fun sampleWater(field: FloatArray, x: Float, y: Float): Float {
        val left = x.toInt()
        val top = y.toInt()
        val across = x - left
        val down = y - top
        val at = top * columns + left
        return (1 - down) * ((1 - across) * field[at] + across * field[at + 1]) +
            down * ((1 - across) * field[at + columns] + across * field[at + columns + 1])
    }

    /**
     * Pressure projection: removes every divergence except the wanted one,
     * so water appears only at springs and disappears only in the drain. The
     * walls are closed.
     */
    private fun project() {
        for (x in 0 until columns) {
            velocityY[x] = 0f
            velocityY[(rows - 1) * columns + x] = 0f
        }
        for (y in 0 until rows) {
            velocityX[y * columns] = 0f
            velocityX[y * columns + columns - 1] = 0f
        }
        for (y in 0 until rows) for (x in 0 until columns) {
            val cell = y * columns + x
            val right = if (x < columns - 1) velocityX[cell + 1] else 0f
            val left = if (x > 0) velocityX[cell - 1] else 0f
            val below = if (y < rows - 1) velocityY[cell + columns] else 0f
            val above = if (y > 0) velocityY[cell - columns] else 0f
            divergence[cell] = .5f * (right - left + below - above) - wanted[cell]
        }
        repeat(PressureIterations) {
            for (y in 0 until rows) for (x in 0 until columns) {
                val cell = y * columns + x
                val left = pressure[if (x > 0) cell - 1 else cell]
                val right = pressure[if (x < columns - 1) cell + 1 else cell]
                val above = pressure[if (y > 0) cell - columns else cell]
                val below = pressure[if (y < rows - 1) cell + columns else cell]
                pressureNext[cell] = (left + right + above + below - divergence[cell]) * .25f
            }
            val previous = pressure
            pressure = pressureNext
            pressureNext = previous
        }
        for (y in 1 until rows - 1) for (x in 1 until columns - 1) {
            val cell = y * columns + x
            velocityX[cell] -= .5f * (pressure[cell + 1] - pressure[cell - 1])
            velocityY[cell] -= .5f * (pressure[cell + columns] - pressure[cell - columns])
        }
    }

    /** For every dye cell, where its ink came from and where it is going, once for all four channels. */
    private fun traceDye(dt: Float) {
        val toWaterIndex = 1f / detail
        for (y in 0 until dyeRows) for (x in 0 until dyeColumns) {
            val cell = y * dyeColumns + x
            // The dye cell's centre in water index coordinates.
            val waterX = ((x + .5f) * toWaterIndex - .5f).coerceIn(0f, columns - 1.001f)
            val waterY = ((y + .5f) * toWaterIndex - .5f).coerceIn(0f, rows - 1.001f)
            val moveX = dt * sampleWater(velocityX, waterX, waterY) * detail
            val moveY = dt * sampleWater(velocityY, waterX, waterY) * detail
            locate(x - moveX, y - moveY, cell, backAt, backAcross, backDown)
            locate(x + moveX, y + moveY, cell, forwardAt, forwardAcross, forwardDown)
        }
    }

    private fun locate(x: Float, y: Float, cell: Int, at: IntArray, across: FloatArray, down: FloatArray) {
        val clampedX = x.coerceIn(0f, dyeColumns - 1.001f)
        val clampedY = y.coerceIn(0f, dyeRows - 1.001f)
        val left = clampedX.toInt()
        val top = clampedY.toInt()
        at[cell] = top * dyeColumns + left
        across[cell] = clampedX - left
        down[cell] = clampedY - top
    }

    /** MacCormack: [field] carried one step, sharp but never beyond its upstream neighbours. Returns the new array. */
    private fun carried(field: FloatArray): FloatArray {
        val row = dyeColumns
        for (cell in 0 until dyeCells) {
            val at = backAt[cell]
            val a = backAcross[cell]
            val d = backDown[cell]
            predicted[cell] = (1 - d) * ((1 - a) * field[at] + a * field[at + 1]) + d * ((1 - a) * field[at + row] + a * field[at + row + 1])
        }
        for (cell in 0 until dyeCells) {
            val at = forwardAt[cell]
            val a = forwardAcross[cell]
            val d = forwardDown[cell]
            val returned = (1 - d) * ((1 - a) * predicted[at] + a * predicted[at + 1]) + d * ((1 - a) * predicted[at + row] + a * predicted[at + row + 1])
            val upstream = backAt[cell]
            val c0 = field[upstream]
            val c1 = field[upstream + 1]
            val c2 = field[upstream + row]
            val c3 = field[upstream + row + 1]
            val low = min(min(c0, c1), min(c2, c3))
            val high = max(max(c0, c1), max(c2, c3))
            corrected[cell] = (predicted[cell] + .5f * (field[cell] - returned)).coerceIn(low, high)
        }
        val result = corrected
        corrected = field
        return result
    }

    /** A uniform fade in the layered modes. The original Zen also absorbs dye at its drain. */
    private fun fadeDye(dt: Float) {
        val fade = exp(-(if (throughFlow) ThroughFlowDissipation else Dissipation) * dt)
        val drain2 = DrainRadius * DrainRadius
        val constant = LinearFade * dt
        for (y in 0 until dyeRows) for (x in 0 until dyeColumns) {
            val cell = y * dyeColumns + x
            val before = amount[cell]
            if (before <= 0f) continue
            val absorbed = if (throughFlow) {
                1f
            } else {
                val dx = (x + .5f) / detail - listenerX
                val dy = (y + .5f) / detail - listenerY
                exp(-Absorption * exp(-(dx * dx + dy * dy) / drain2) * dt)
            }
            val after = max(0f, before * fade * absorbed - constant)
            val keep = after / before
            amount[cell] = after
            red[cell] *= keep
            green[cell] *= keep
            blue[cell] *= keep
        }
    }

    /** Lets [plumes] flow for [seconds] at 30 steps a second: how a scene looks once it is going. */
    fun settle(plumes: List<Plume>, seconds: Float = 8f) {
        var time = 0f
        while (time < seconds) {
            step(plumes, 1f / 30)
            time += 1f / 30
        }
    }

    /** Total pigment in the water, per water cell of area. */
    fun totalInk(): Float = amount.sum() / (detail * detail)

    /** Pigment within [radius] water cells of ([x], [y]), per water cell of area. */
    fun inkNear(x: Float, y: Float, radius: Float): Float {
        var sum = 0f
        for (row in 0 until dyeRows) for (column in 0 until dyeColumns) {
            val dx = (column + .5f) / detail - x
            val dy = (row + .5f) / detail - y
            if (dx * dx + dy * dy <= radius * radius) sum += amount[row * dyeColumns + column]
        }
        return sum / (detail * detail)
    }

    /** Fastest water anywhere (cells/s). */
    fun fastest(): Float {
        var top = 0f
        for (cell in 0 until cells) top = max(top, sqrt(velocityX[cell] * velocityX[cell] + velocityY[cell] * velocityY[cell]))
        return top
    }

    /**
     * Writes the pigment as premultiplied RGBA bytes, one pixel per dye
     * cell, row by row. Alpha is the ink's coverage. [tint], when set,
     * replaces every ink's color.
     */
    fun writePixels(into: ByteArray, tint: Color? = null) {
        for (cell in 0 until dyeCells) {
            val ink = amount[cell]
            val base = cell * 4
            if (ink < 1e-4f) {
                into.fill(0, base, base + 4)
                continue
            }
            val alpha = 1f - exp(-ink * Opacity)
            val r = tint?.red ?: (red[cell] / ink)
            val g = tint?.green ?: (green[cell] / ink)
            val b = tint?.blue ?: (blue[cell] / ink)
            into[base] = byte(r * alpha)
            into[base + 1] = byte(g * alpha)
            into[base + 2] = byte(b * alpha)
            into[base + 3] = byte(alpha)
        }
    }

    private fun byte(value: Float): Byte = (value.coerceIn(0f, 1f) * 255f + .5f).toInt().toByte()

    companion object {
        /** Longer frames are split no further: the water just moves less. */
        const val MaxStep = 1f / 20

        private const val Inset = 2.5f
        private const val SpringRadius = 3.2f
        private const val DrainRadius = 3.5f
        private const val DyeCore = .55f
        private const val JetSpeed = 12f
        private const val SpringFlow = 260f
        private const val InkRate = 7f
        private const val DragShare = .7f
        private const val MaxDrag = 80f
        private const val Confinement = 7f
        private const val Damping = .2f
        private const val LinearDrag = .4f
        private const val Dissipation = .08f
        private const val ThroughFlowDissipation = .22f
        private const val LinearFade = .004f
        private const val Absorption = 3f
        private const val Opacity = 1.3f
        private const val PressureIterations = 30

        /** Pixels per water cell for a canvas [width] × [height] pixels, the longer side [cellsAcross] cells. */
        fun cellSize(width: Float, height: Float, cellsAcross: Int = 112): Float = max(width, height) / cellsAcross
    }
}
