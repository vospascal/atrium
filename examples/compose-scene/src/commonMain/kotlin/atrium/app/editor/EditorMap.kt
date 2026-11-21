package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.withFrameMillis
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.ui.Alignment
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.unit.dp
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.sp
import atrium.engine.EmitterKind
import atrium.engine.Position
import atrium.engine.Preset
import kotlin.math.PI
import kotlin.math.atan2
import kotlin.math.roundToInt
import atrium.app.ink.LocalInkTint
import atrium.app.ui.LocalPlayerColors
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.Point
import atrium.app.session.Session
import atrium.app.ink.parseColor

/**
 * Every sound of the playing scene from above, in the player's paper style.
 * Click a sound to select it, with Shift or ⌘ to add it to the others, drag
 * it to move it and every other selected sound with it, click the ground to
 * select nothing. The listener lies on top of every sound, so it can always be
 * picked. Hold Alt while dragging and copies land where the drag ends.
 * A sound is black and white until it is selected. Its circle, or
 * its wedge when it is focused, is how far its level carries, and a ring
 * travels outward while it is heard. An ambient sound is bands arriving from
 * the side its anchor lies on, and can be grabbed near that anchor. The
 * selected focused sound has a handle at the end of its facing line: drag it
 * to turn the sound.
 */
@Composable
internal fun EditorMap(
    session: Session,
    strengths: List<Float>,
    selection: Selection,
    onSelection: (Selection) -> Unit,
    carry: Carry<Preset>,
    modifier: Modifier = Modifier,
) {
    val colors = LocalPlayerColors.current
    val tint = LocalInkTint.current
    val text = rememberTextMeasurer()
    val description = session.description
    val telemetry = session.telemetry
    val listener = description?.listener ?: Position(0f, 0f, 0f)
    // Where the listener stood when the scene opened: the map's fixed origin, so the listener
    // itself can be moved over it.
    val origin = remember(session.engine, description == null) { listener }
    var heldListener by remember(session.engine) { mutableStateOf<HeldListener?>(null) }
    val liveFacing = telemetry?.listenerFacing
    val listenerAt = heldListener?.offset ?: Point(listener.x - origin.x, listener.y - origin.y)
    val listenerHeading = heldListener?.heading ?: liveFacing?.let { atan2(it.y, it.x) } ?: (PI.toFloat() / 2f)
    val listenerNow = rememberUpdatedState(listenerAt to listenerHeading)
    var dragged by remember(session.engine) { mutableStateOf<Dragged?>(null) }
    var turned by remember(session.engine) { mutableStateOf<Turned?>(null) }
    val echo = remember(session.engine) { DragEcho() }

    // Hand a dragged sound back to telemetry once the engine reports it where it was dropped.
    LaunchedEffect(telemetry) {
        val held = dragged?.takeIf { !it.copying } ?: return@LaunchedEffect
        val reported = telemetry?.sources?.firstOrNull { it.slot == held.lead }?.position ?: return@LaunchedEffect
        if (echo.accepts(Point(reported.x - origin.x, reported.y - origin.y))) dragged = null
    }

    val markers = description?.sources.orEmpty().mapIndexed { index, sound ->
        val live = telemetry?.sources?.firstOrNull { it.slot == sound.slot }
        val at = live?.position ?: sound.position
        val held = dragged?.takeIf { !it.copying }?.offsets?.get(sound.slot)
        val facing = live?.facing ?: sound.facing
        Marker(
            sound.slot,
            sound.id,
            sound.name,
            held ?: Point(at.x - origin.x, at.y - origin.y),
            height = at.z,
            // Black and white until it is selected; then its own ink.
            if (sound.id in selection) tint ?: parseColor(sound.color) else colors.foreground,
            reach = reachOf(sound.spl, sound.referenceDistance),
            ambient = sound.emitterKind == EmitterKind.FIELD,
            beamHalfAngle = sound.beamHalfAngle,
            facing = turned?.takeIf { it.slot == sound.slot }?.heading ?: atan2(facing.y, facing.x),
            size = sound.size,
            strength = strengths.getOrElse(index) { 0f },
            silent = session.mix.silences(sound.id),
            sounding = live == null || live.take != null,
        )
    }
    // Sized by where sounds were placed, so the view holds still while one is dragged or moves by itself.
    val reach = reachFor(description?.sources.orEmpty().map { Point(it.position.x - origin.x, it.position.y - origin.y) })
    // Zoomed and panned by the listener; a new scene starts showing all of it.
    var frame by remember(session.engine) { mutableStateOf(MapFrame()) }
    val framing = rememberUpdatedState(frame)
    val focus = remember { FocusRequester() }
    val current = rememberUpdatedState(markers)
    val select = rememberUpdatedState(onSelection)
    val chosen = rememberUpdatedState(selection)
    val keys = remember { HeldKeys() }
    // The loudness layer: off until asked for, then the whole scene, or the one selected sound alone.
    var loud by remember { mutableStateOf(false) }

    // Seconds since the map appeared: what the travelling rings and drifting bands move by.
    val beat by produceState(0f) {
        val start = withFrameMillis { it }
        while (true) withFrameMillis { value = (it - start) / 1000f }
    }

    var size by remember { mutableStateOf(androidx.compose.ui.geometry.Size.Zero) }
    // Whether a drag that began on empty ground is moving the view.
    var panning by remember { mutableStateOf(false) }

    /** Move the selected sounds, or the listener, over the ground by [step] metres. */
    fun nudge(step: Point): Boolean {
        if (chosen.value.single == ListenerId) {
            session.moveListener(Position(listener.x + step.x, listener.y + step.y, listener.z))
            session.placed()
            return true
        }
        val sounds = description?.sources.orEmpty().filter { it.id in chosen.value }
        if (sounds.isEmpty()) return false
        session.move(sounds.associate { it.slot to Position(it.position.x + step.x, it.position.y + step.y, it.position.z) })
        session.placed()
        return true
    }

    /** Where the sounds of [held] are in the world, each at its own height. */
    fun placesOf(held: Dragged): Map<UShort, Position> = held.offsets.mapValues { (slot, at) ->
        Position(origin.x + at.x, origin.y + at.y, current.value.firstOrNull { it.slot == slot }?.height ?: 0f)
    }

    val view = frame.view(size, reach)
    val only = description?.sources?.firstOrNull { it.id == selection.single }
    // Worked out again when a sound, the mix or the view changes: not at every point of a drag,
    // during which the scene's description stands still.
    // The loudest band is the loudest sound shown at 1 m, so the colours hold still while the map is moved.
    val loudest = loudestBandTop((only?.let(::listOf) ?: description?.sources.orEmpty().filterNot { session.mix.silences(it.id) }).maxOfOrNull { it.spl } ?: 0f)
    val loudness = remember(loud, description, session.mix, frame, size, reach, only?.slot) {
        if (!loud) null else loudnessPatch(view, size, origin, listener.z)?.let { session.loudness(it.from, it.to, it.columns, it.rows, only?.slot) }
    }

    Box(modifier) {
    Canvas(
        Modifier.fillMaxSize()
            .onSizeChanged { size = androidx.compose.ui.geometry.Size(it.width.toFloat(), it.height.toFloat()) }
            // A preset dropped on the map is added where it was let go, at the listener's ear height.
            .carryTarget(carry) { preset, local ->
                val at = framing.value.view(size, reach).toWorld(local)
                session.add(preset.path, Position(origin.x + at.x, origin.y + at.y, listener.z)) { id -> onSelection(Selection(setOf(id))) }
            }
            .clipToBounds()
            .then(keys.watch)
            // The arrow keys nudge the selected sound once the map has been clicked.
            .focusRequester(focus)
            .focusable()
            .onKeyEvent { event ->
                if (event.type != KeyEventType.KeyDown) return@onKeyEvent false
                val step = nudgeFor(event.key, coarse = event.isShiftPressed) ?: return@onKeyEvent false
                nudge(step)
            }
            .zoomOnScroll(session.engine, reach) { factor, about ->
                frame = framing.value.zoomedAbout(factor, about, size, reach)
            }
            .pointerInput(session.engine, reach) {
                detectTapGestures { at ->
                    focus.requestFocus()
                    val view = framing.value.view(size, reach)
                    select.value(chosen.value.clicked(clickedAt(view, at, density, current.value, listenerNow.value.first), keys.adding))
                }
            }
            .pointerInput(session.engine, reach) {
                var view: TopDown? = null
                detectDragGestures(
                    onDragStart = { at ->
                        focus.requestFocus()
                        val top = framing.value.view(size, reach)
                        view = top
                        val (listenerOffset, listenerFacing) = listenerNow.value
                        when (val grab = grabAt(top, at, density, current.value, chosen.value, listenerOffset, listenerFacing)) {
                            is Grab.Facing -> turned = Turned(grab.marker.slot, grab.marker.facing)
                            Grab.ListenerFacing -> heldListener = HeldListener(listenerOffset, listenerFacing, turning = true)
                            Grab.Listener -> {
                                select.value(Selection(setOf(ListenerId)))
                                heldListener = HeldListener(listenerOffset, listenerFacing, turning = false)
                            }
                            is Grab.Sound -> {
                                val marker = grab.marker
                                // One of several selected sounds carries them all; any other sound is taken alone.
                                val carried = chosen.value.grabbed(marker.id)
                                if (carried != chosen.value) select.value(carried)
                                val places = current.value.filter { it.id in carried }.associate { it.slot to it.offset }
                                val copying = keys.copying
                                if (!copying) echo.move()
                                dragged = Dragged(marker.slot, places, places, copying)
                            }
                            Grab.Ground -> panning = true
                        }
                    },
                    onDrag = { change, distance ->
                        if (panning) {
                            frame = framing.value.pannedBy(distance, size, reach)
                            return@detectDragGestures
                        }
                        val top = view
                        val holding = heldListener
                        if (holding != null && top != null) {
                            if (holding.turning) {
                                val toward = groundDirection(top.toScreen(holding.offset), change.position)
                                if (toward.hasGroundBearing()) {
                                    heldListener = holding.copy(heading = atan2(toward.y, toward.x))
                                    session.faceListener(Position(toward.x, toward.y, 0f))
                                }
                            } else {
                                val at = top.toWorld(change.position)
                                heldListener = holding.copy(offset = at)
                                session.moveListener(Position(origin.x + at.x, origin.y + at.y, listener.z))
                            }
                            return@detectDragGestures
                        }
                        val turning = turned
                        if (turning != null && top != null) {
                            val sound = current.value.firstOrNull { it.slot == turning.slot }
                            val toward = sound?.let { groundDirection(top.toScreen(it.offset), change.position) }
                            if (toward != null && toward.hasGroundBearing()) {
                                turned = turning.copy(heading = atan2(toward.y, toward.x))
                                session.face(turning.slot, Position(toward.x, toward.y, 0f))
                            }
                            return@detectDragGestures
                        }
                        val held = dragged
                        if (held != null && top != null) {
                            val moved = held.movedTo(top.toWorld(change.position))
                            dragged = moved
                            if (!moved.copying) session.move(placesOf(moved))
                        }
                    },
                    onDragEnd = {
                        dragged?.let { held ->
                            if (held.copying) {
                                // The copies land where the drag ended, and are what is selected after.
                                val originals = description?.sources.orEmpty().filter { it.slot in held.offsets }
                                session.duplicate(originals, placesOf(held)) { ids -> select.value(Selection(ids)) }
                                dragged = null
                            } else {
                                echo.release(held.offsets.getValue(held.lead))
                            }
                        }
                        heldListener = null
                        turned = null
                        panning = false
                        session.placed()
                    },
                    onDragCancel = {
                        dragged?.let { held -> if (held.copying) dragged = null else echo.release(held.offsets.getValue(held.lead)) }
                        heldListener = null
                        turned = null
                        panning = false
                        session.placed()
                    },
                )
            },
    ) {
        // The loudness of the ground lies under everything.
        loudness?.let { drawLoudness(it, loudest, view, origin, colors.dark) }
        // The listener: rings of distance, ambient bands and bearings are measured from it.
        val centre = view.toScreen(listenerAt)

        ringsWithin(reach).forEach { metres ->
            drawCircle(colors.hairline, metres * view.pixelsPerMetre, centre, style = Stroke(width = density * .8f))
            val label = text.measure("${metres.roundToInt()} m", TextStyle(color = colors.muted, fontSize = 11.sp))
            drawText(label, topLeft = centre + Offset(density * 4f, -metres * view.pixelsPerMetre - label.size.height))
        }
        val north = text.measure("N", TextStyle(color = colors.muted, fontSize = 11.sp))
        drawText(north, topLeft = Offset(centre.x - north.size.width / 2f, density * 6f))

        // Ambient washes first, so points and wedges stay readable on top of them.
        markers.filter { it.ambient }.forEach { drawAmbient(it, listenerAt, view, colors, text, it.id in selection, beat) }
        // Reaches under every point, so a small sound inside a large one can still be seen and picked.
        markers.filterNot { it.ambient }.sortedByDescending { it.reach }.forEach { drawReach(view.toScreen(it.offset), it, view, beat, it.id in selection) }

        // The room's loudspeakers, under the sounds: filled by what each plays, an outline when the output passes them by.
        // On headphones none plays, and the room's speakers have nothing to do with what is heard, so none is drawn.
        val inUse = telemetry?.channelsInUse.orEmpty().toSet()
        if (inUse.isNotEmpty()) description?.speakers.orEmpty().forEach { speaker ->
            val offset = Point(speaker.position.x - origin.x, speaker.position.y - origin.y)
            val level = telemetry?.peaksDb?.getOrNull(speaker.channel.toInt())?.let(::meterOf) ?: 0f
            drawSpeaker(view.toScreen(offset), centre, speaker.name, speaker.channel in inUse, level, colors, text)
        }

        markers.filterNot { it.ambient }.forEach { marker ->
            val at = view.toScreen(marker.offset)
            // The bearing and the facing handle belong to one sound; with several selected they would crowd the map.
            if (marker.id == selection.single) drawBearingLine(centre, at, Point(marker.offset.x - listenerAt.x, marker.offset.y - listenerAt.y), colors, text)
            if (marker.id == selection.single && marker.turnable) drawFacingHandle(at, marker, colors)
            drawPoint(at, marker, colors, text, marker.id in selection)
        }

        // The listener last of all that can be picked, so it is never under a sound.
        drawListener(centre, listenerHeading.takeIf { liveFacing != null || heldListener != null }, colors, selection.single == ListenerId)

        // Where the copies will land while Alt is held: an open dot each, joined to its original.
        dragged?.takeIf { it.copying }?.let { held ->
            held.offsets.forEach { (slot, offset) ->
                val original = markers.firstOrNull { it.slot == slot } ?: return@forEach
                val at = view.toScreen(offset)
                drawLine(original.color.copy(alpha = .4f), view.toScreen(original.offset), at, strokeWidth = density, pathEffect = PathEffect.dashPathEffect(floatArrayOf(density * 4f, density * 4f)))
                drawCircle(colors.backdrop, density * 6f, at)
                drawCircle(original.color, density * 6f, at, style = Stroke(width = density * 1.6f))
            }
        }
    }
    Row(Modifier.align(Alignment.BottomStart).padding(12.dp), verticalAlignment = Alignment.Bottom) {
        ToolButton("Loudness", strong = loud) { loud = !loud }
        if (loud) {
            Spacer(Modifier.width(10.dp))
            LoudnessLegend(of = only?.name ?: "everything heard", top = loudest)
        }
    }
    ZoomControls(frame, Modifier.align(Alignment.BottomEnd).padding(12.dp)) { frame = it(size, reach) }
    }
}

/** Zoom out, the zoom as a percentage, zoom in, and Fit, which shows the whole scene again. */
@Composable
private fun ZoomControls(frame: MapFrame, modifier: Modifier, onFrame: ((Size, Float) -> MapFrame) -> Unit) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        ToolButton("−") { onFrame { size, reach -> frame.zoomedAbout(1f / ZoomStep, Offset(size.width / 2f, size.height / 2f), size, reach) } }
        PaperText("${(frame.zoom * 100).roundToInt()} %", 13, Muted, modifier = Modifier.padding(horizontal = 8.dp))
        ToolButton("+") { onFrame { size, reach -> frame.zoomedAbout(ZoomStep, Offset(size.width / 2f, size.height / 2f), size, reach) } }
        Spacer(Modifier.width(6.dp))
        ToolButton("Fit", enabled = frame != MapFrame()) { onFrame { _, _ -> MapFrame() } }
    }
}

/** How much one press of a zoom button zooms. */
private const val ZoomStep = 1.5f
