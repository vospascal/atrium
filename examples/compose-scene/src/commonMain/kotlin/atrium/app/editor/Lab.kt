package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEvent
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isMetaPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import atrium.app.session.LabBench
import atrium.app.ui.Backdrop
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ui.Highlight
import atrium.app.ui.Muted
import atrium.app.ui.PaperLevel
import atrium.app.ui.PaperText
import atrium.app.ui.ScrollArea
import atrium.app.ui.SmallChip
import atrium.engine.LabCandidate
import atrium.engine.LabFader
import atrium.engine.LabLayer
import atrium.engine.LabState
import atrium.engine.LabTrim
import atrium.engine.LabWaveform
import kotlin.math.abs
import kotlin.math.roundToInt

/** A mixer fader's range (dB): 0 is the layer as the generator gives it. */
private const val QuietestDb = -24f
private const val LoudestDb = 12f

/** How close to one of the wind's moves (px) a dragged cue snaps onto it. */
private const val SnapPixels = 12f

/** The shortest passage the markers hold (s). */
private const val ShortestPassage = .25f

/**
 * The generator lab: the soft wind tuned against a recording, each round asking one question.
 * The recording's waveform with the passage's markers, the seeded run's under it with markers
 * that snap to the gusts the wind's driver reports; which candidate is heard, and the Lab's own
 * playback with its playhead; then, scrolling, the working wind's mixer, its behaviour, notes
 * and the verdict. With the Lab focused, space plays and pauses it, and R, P and W switch to
 * Reference, Previous and Working; a text field keeps its own keys.
 */
@Composable
internal fun LabPanel(lab: LabBench, modifier: Modifier = Modifier) {
    LaunchedEffect(lab) {
        while (true) withFrameMillis { lab.refresh() }
    }
    val state = lab.state
    val focus = remember { FocusRequester() }
    Column(
        modifier.fillMaxWidth()
            .focusRequester(focus)
            .onFocusChanged { lab.focused = it.hasFocus }
            .focusable()
            // A press anywhere in the Lab gives it the keys; a text field pressed takes them after.
            .pointerInput(Unit) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                    focus.requestFocus()
                }
            }
            .onKeyEvent { event -> labKey(event, lab) }
            .padding(start = 14.dp, end = 14.dp, top = 6.dp),
    ) {
        when {
            state == null -> PaperText("Nothing playing", 13, Muted)
            lab.recording == null -> EmptyLab(lab, state)
            else -> Workbench(lab, state)
        }
    }
}

/** Space, R, P and W, while the Lab has the keys. */
private fun labKey(event: KeyEvent, lab: LabBench): Boolean {
    if (event.type != KeyEventType.KeyDown || event.isMetaPressed || event.isCtrlPressed) return false
    when (event.key) {
        Key.Spacebar -> lab.togglePlay()
        Key.R -> lab.hear(LabCandidate.REFERENCE)
        Key.P -> lab.hear(LabCandidate.PREVIOUS)
        Key.W -> lab.hear(LabCandidate.WORKING)
        else -> return false
    }
    return true
}

/** Before a reference is open: the sound, and the way to a recording or the last session. */
@Composable
private fun EmptyLab(lab: LabBench, state: LabState) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        PaperText(state.sound, 14, weight = FontWeight.Bold)
        SmallChip("Open recording…") { chooseRecording()?.let(lab::openRecording) }
        if (state.savedSession) SmallChip("Reopen last session") { lab.reopen() }
    }
    Spacer(Modifier.height(6.dp))
    StatusLine(lab, state)
}

/** The Lab with a recording open. */
@Composable
private fun Workbench(lab: LabBench, state: LabState) {
    val recording = lab.recording ?: return
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            PaperText(state.sound, 14, weight = FontWeight.Bold)
            Spacer(Modifier.width(12.dp))
            KeptField(state.question, "What are you listening for? Does the gust fade naturally?", Modifier.weight(1f), lab::question)
        }
        Row(Modifier.fillMaxWidth().padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val marked = state.passage
            PaperText(recording.file.substringAfterLast('/') + (marked?.let { " · ${it.name}" } ?: ""), 12, Muted, singleLine = true)
            Spacer(Modifier.weight(1f))
            EditedField(lab.passageName, "Passage name", Modifier.width(110.dp)) { lab.passageName = it }
            SmallChip("Open recording…") { chooseRecording()?.let(lab::openRecording) }
            val moved = marked == null || marked.referenceFrom != lab.referenceFrom || marked.referenceTo != lab.referenceTo ||
                marked.generatorCue != lab.generatorCue || marked.name != lab.passageName
            SmallChip("Mark passage", selected = moved) { lab.markPassage() }
        }
        StatusLine(lab, state)
        Lanes(lab, state)
        if (state.passage == null) {
            PaperText("Drag the markers round a gust with some lead-in and tail, and the run's start onto a gust of its own; then Mark passage.", 12, Muted)
            return@Column
        }
        CandidateBar(lab, state)
        PlaybackBar(lab, state)
        Rule()
        ScrollArea(Modifier.weight(1f)) {
            MatchReport(state)
            Mixer(lab, state)
            Behaviour(lab, state)
            NotesAndVerdict(lab, state)
            Spacer(Modifier.height(16.dp))
        }
    }
}

/** What is under way, what went wrong, and whether the marked gust still lines up. */
@Composable
private fun StatusLine(lab: LabBench, state: LabState) {
    val line = lab.busy ?: lab.failure ?: if (!state.linesUp) {
        "The marked gust no longer lines up: a timing setting changed the gusts. Compare spacing and variation with Run freely."
    } else null
    if (line != null) PaperText(line, 12, if (lab.failure != null && lab.busy == null) Foreground else Muted)
}

/** The recording's waveform and the seeded run's, each with its passage's markers. */
@Composable
private fun Lanes(lab: LabBench, state: LabState) {
    val recording = lab.recording ?: return
    val marked = state.passage
    val heard = state.heard ?: state.asked
    val playhead = state.playheadSeconds.takeIf { marked != null && (state.playing || it > 0f) }
    LaneRow("Reference") {
        WaveLane(
            recording.waveform,
            from = lab.referenceFrom,
            to = lab.referenceTo,
            playhead = playhead?.let { (marked?.referenceFrom ?: 0f) + it },
            playheadHeard = heard == LabCandidate.REFERENCE,
            moves = emptyList(),
            modifier = Modifier.weight(1f),
        ) { start, seconds ->
            if (start) lab.referenceFrom = seconds.coerceAtMost(lab.referenceTo - ShortestPassage).coerceAtLeast(0f)
            else lab.referenceTo = seconds.coerceAtLeast(lab.referenceFrom + ShortestPassage)
        }
    }
    val run = lab.run
    LaneRow(
        "Seed ${state.seed}",
        trailing = {
            SmallChip("−") { if (state.seed > 1u) lab.seed(state.seed - 1u) }
            SmallChip("+") { lab.seed(state.seed + 1u) }
        },
    ) {
        if (run == null) {
            PaperText("Running the wind…", 12, Muted, modifier = Modifier.weight(1f))
        } else {
            val length = lab.passageSeconds
            WaveLane(
                run.waveform,
                from = lab.generatorCue,
                to = lab.generatorCue + length,
                playhead = playhead?.let { (marked?.generatorCue ?: 0f) + it },
                playheadHeard = heard != LabCandidate.REFERENCE,
                moves = run.moves.map { it.seconds },
                modifier = Modifier.weight(1f),
            ) { start, seconds ->
                // Both markers move the passage: it keeps the recording's length.
                lab.generatorCue = (if (start) seconds else seconds - length).coerceIn(0f, run.waveform.seconds)
            }
        }
    }
}

@Composable
private fun LaneRow(label: String, trailing: (@Composable RowScope.() -> Unit)? = null, lane: @Composable RowScope.() -> Unit) {
    Row(Modifier.fillMaxWidth().height(44.dp).padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(Modifier.width(118.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            PaperText(label, 12, Muted, singleLine = true)
            trailing?.invoke(this)
        }
        lane()
    }
}

/**
 * A waveform across its whole length, the passage between [from] and [to] drawn darker with a
 * marker at each end, the [moves] as ticks along the top, and the Lab's [playhead]. Dragging
 * near a marker moves it ([onMarker] gets which, and the time); a marker dragged near a move
 * snaps onto it.
 */
@Composable
private fun WaveLane(
    waveform: LabWaveform,
    from: Float,
    to: Float,
    playhead: Float?,
    playheadHeard: Boolean,
    moves: List<Float>,
    modifier: Modifier = Modifier,
    onMarker: (start: Boolean, seconds: Float) -> Unit,
) {
    val ink = Foreground
    val faint = Hairline
    val muted = Muted
    val mark = Highlight
    val ends = rememberUpdatedState(from to to)
    val snapTo = rememberUpdatedState(moves)
    val move = rememberUpdatedState(onMarker)
    val seconds = waveform.seconds.coerceAtLeast(.001f)
    val peak = remember(waveform) {
        maxOf(waveform.maxima.maxOrNull() ?: 0f, -(waveform.minima.minOrNull() ?: 0f)).coerceAtLeast(1e-6f)
    }
    var grabbing by remember { mutableStateOf(true) }
    Canvas(
        modifier.fillMaxSize().pointerInput(waveform) {
            fun timeAt(x: Float): Float {
                val raw = (x / size.width).coerceIn(0f, 1f) * seconds
                val nearest = snapTo.value.minByOrNull { abs(it - raw) } ?: return raw
                return if (abs(nearest - raw) / seconds * size.width <= SnapPixels) nearest else raw
            }
            detectDragGestures(
                onDragStart = { offset ->
                    val (start, end) = ends.value
                    val at = offset.x / size.width * seconds
                    grabbing = abs(at - start) <= abs(at - end)
                    move.value(grabbing, timeAt(offset.x))
                },
            ) { change, _ ->
                change.consume()
                move.value(grabbing, timeAt(change.position.x))
            }
        },
    ) {
        val middle = size.height / 2
        val count = waveform.maxima.size
        val startX = from / seconds * size.width
        val endX = to / seconds * size.width
        drawRect(faint.copy(alpha = .35f), Offset(startX, 0f), androidx.compose.ui.geometry.Size((endX - startX).coerceAtLeast(0f), size.height))
        for (index in 0 until count) {
            val x = (index + .5f) / count * size.width
            val inside = x in startX..endX
            val top = middle - waveform.maxima[index] / peak * middle * .9f
            val bottom = middle - waveform.minima[index] / peak * middle * .9f
            drawLine(if (inside) ink else muted, Offset(x, top), Offset(x, bottom.coerceAtLeast(top + .5f)), 1f)
        }
        for (start in moves) {
            val x = start / seconds * size.width
            drawLine(mark, Offset(x, 0f), Offset(x, size.height * .22f), 2.dp.toPx())
        }
        for (x in listOf(startX, endX)) {
            drawLine(ink, Offset(x, 0f), Offset(x, size.height), 1.5.dp.toPx())
            val triangle = Path().apply {
                moveTo(x, size.height - 7.dp.toPx())
                lineTo(x - 4.dp.toPx(), size.height)
                lineTo(x + 4.dp.toPx(), size.height)
                close()
            }
            drawPath(triangle, ink)
        }
        if (playhead != null) {
            val x = playhead / seconds * size.width
            drawLine(if (playheadHeard) mark else muted, Offset(x, 0f), Offset(x, size.height), if (playheadHeard) 2.dp.toPx() else 1.dp.toPx())
        }
    }
}

/** Which candidate is heard, the three one press apart, and each generator's trim. */
@Composable
private fun CandidateBar(lab: LabBench, state: LabState) {
    Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        val hearing = when {
            !state.playing -> "Paused · ${state.asked.label}"
            state.heard == null -> "Switching to ${state.asked.label}"
            else -> "Hearing ${state.heard?.label}"
        }
        PaperText(hearing, 13, weight = FontWeight.Bold, modifier = Modifier.width(170.dp), singleLine = true)
        LabCandidate.entries.forEach { candidate ->
            val key = candidate.label.first()
            SmallChip("${candidate.label} ($key)", selected = state.asked == candidate, enabled = state.measuring == null) { lab.hear(candidate) }
        }
        Spacer(Modifier.width(4.dp))
        state.trims.filter { it.candidate != LabCandidate.REFERENCE }.forEach { trim ->
            PaperText("${trim.candidate.label} ${trimLine(trim)}", 12, if (trim.rematchNeeded) Foreground else Muted, singleLine = true)
        }
    }
}

/** The Lab's own playback, apart from the scene's ruler, and level matching. */
@Composable
private fun PlaybackBar(lab: LabBench, state: LabState) {
    Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SmallChip(if (state.playing) "Pause" else "Play") { lab.togglePlay() }
        SmallChip("Loop", selected = state.looping) { lab.loop(!state.looping) }
        SmallChip("Replay gust", enabled = state.measuring == null) { lab.replay() }
        PaperText("Lab playhead ${clock(state.playheadSeconds)} / ${clock(state.loopSeconds)}", 12, Muted, singleLine = true)
        Spacer(Modifier.weight(1f))
        val measuring = state.measuring
        if (measuring != null) {
            PaperText("Matching · ${measuring.label} alone · ${(state.matchProgress * 100).roundToInt()} %", 12, Foreground, singleLine = true)
        } else {
            SmallChip("Match levels") { lab.matchLevels() }
        }
        SmallChip("Hold level match", selected = state.holdLevelMatch) { lab.holdLevelMatch(!state.holdLevelMatch) }
    }
}

/** What the last match measured: each generator's distance from the reference, and how even the passage is. */
@Composable
private fun MatchReport(state: LabState) {
    val measured = state.trims.filter { it.differenceDb != null && it.candidate != LabCandidate.REFERENCE }
    if (measured.isEmpty()) return
    val parts = measured.joinToString(" · ") { trim ->
        val halves = trim.halvesApartDb?.let { ", halves ${decibels(it, signed = false)} apart" } ?: ""
        "${trim.candidate.label} measured ${decibels(trim.differenceDb ?: 0f)} from the reference$halves"
    }
    val reference = state.trims.firstOrNull { it.candidate == LabCandidate.REFERENCE }?.halvesApartDb
    val referenceHalves = reference?.let { " · Reference halves ${decibels(it, signed = false)} apart" } ?: ""
    PaperText("Last match, A-weighted while sounding: $parts$referenceHalves", 12, Muted, modifier = Modifier.padding(vertical = 4.dp))
}

/** The working wind's five layers: a fader, solo and mute each; undo steps back through changes. */
@Composable
private fun Mixer(lab: LabBench, state: LabState) {
    Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        PaperText("Mix · Editing Working", 13, weight = FontWeight.Bold)
        Spacer(Modifier.weight(1f))
        SmallChip("Undo mix", enabled = state.canUndoMix) { lab.undoMix() }
    }
    state.mix.forEach { fader -> FaderRow(fader, lab) }
}

@Composable
private fun FaderRow(fader: LabFader, lab: LabBench) {
    Row(Modifier.fillMaxWidth().height(30.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        PaperText(fader.layer.label, 12, modifier = Modifier.width(70.dp))
        PaperLevel(
            (fader.levelDb - QuietestDb) / (LoudestDb - QuietestDb),
            onChange = { along ->
                val decibels = ((QuietestDb + along * (LoudestDb - QuietestDb)) * 2).roundToInt() / 2f
                lab.fader(fader.copy(levelDb = decibels))
            },
            modifier = Modifier.weight(1f),
            step = .5f / (LoudestDb - QuietestDb),
        )
        // Pressing the level puts the layer back as the generator gives it.
        PaperText(
            decibels(fader.levelDb, signed = true, whole = true),
            12,
            modifier = Modifier.width(52.dp).clickable(role = Role.Button) { lab.fader(fader.copy(levelDb = 0f)) },
            singleLine = true,
        )
        SmallChip("Solo", selected = fader.solo) { lab.fader(fader.copy(solo = !fader.solo)) }
        SmallChip("Mute", selected = fader.mute) { lab.fader(fader.copy(mute = !fader.mute)) }
    }
}

/** The generator's behaviour settings and actions; closed until opened. */
@Composable
private fun Behaviour(lab: LabBench, state: LabState) {
    PaperText(
        (if (lab.behaviourOpen) "▾ " else "▸ ") + "Behaviour",
        13,
        weight = FontWeight.Bold,
        modifier = Modifier.padding(top = 8.dp, bottom = 2.dp).clickable(role = Role.Button) { lab.behaviourOpen = !lab.behaviourOpen },
    )
    if (!lab.behaviourOpen) return
    state.behaviour.forEach { setting ->
        Row(Modifier.fillMaxWidth().height(30.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            PaperText(setting.label, 12, modifier = Modifier.width(120.dp), singleLine = true)
            val range = (setting.high - setting.low).coerceAtLeast(1e-6f)
            PaperLevel(
                (setting.value - setting.low) / range,
                onChange = { along -> lab.setting(setting.name, setting.low + along * range, setting.kind) },
                modifier = Modifier.weight(1f),
            )
            PaperText(((setting.value * 100).roundToInt() / 100f).toString(), 12, modifier = Modifier.width(52.dp), singleLine = true)
        }
        PaperText(setting.description, 11, Muted)
    }
    Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SmallChip("Run freely", selected = state.runFreely) { lab.runFreely(!state.runFreely) }
        PaperText("The wind runs on over the rest of the recording, to compare the spacing and variation of gusts rather than one gust.", 11, Muted)
    }
}

/** Notes pinned to the moment heard, the verdict, and saving the session. */
@Composable
private fun NotesAndVerdict(lab: LabBench, state: LabState) {
    var note by remember { mutableStateOf("") }
    Row(Modifier.fillMaxWidth().padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        EditedField(note, "What did you hear, and where?", Modifier.weight(1f)) { note = it }
        SmallChip("Pin note", enabled = note.isNotBlank()) {
            lab.pinNote(note)
            note = ""
        }
    }
    state.notes.forEach { pinned ->
        PaperText(
            "${clock(pinned.atSeconds)} · ${pinned.heard.label} at ${clock(pinned.momentSeconds)} — ${pinned.text}",
            12,
            Muted,
        )
    }
    Spacer(Modifier.height(6.dp))
    KeptField(state.verdict, "Verdict, in your own words: the fade sounds better, but the onset is still too sharp", Modifier.fillMaxWidth(), lab::verdict)
    Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SmallChip("Save session") { lab.save() }
        lab.savedTo?.let { PaperText("Saved to $it", 12, Muted) }
    }
}

/** A text field kept by its owner, who sees every change. */
@Composable
private fun EditedField(value: String, placeholder: String, modifier: Modifier = Modifier, onChange: (String) -> Unit) {
    var focused by remember { mutableStateOf(false) }
    val line = Hairline
    BasicTextField(
        value,
        onValueChange = onChange,
        singleLine = true,
        textStyle = TextStyle(color = Foreground, fontSize = 12.sp),
        cursorBrush = SolidColor(Foreground),
        decorationBox = { field ->
            Box(Modifier.border(1.dp, line, RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 3.dp)) {
                if (value.isEmpty() && !focused) PaperText(placeholder, 12, Muted, singleLine = true) else field()
            }
        },
        modifier = modifier.onFocusChanged { focused = it.isFocused },
    )
}

/** A text field whose text is handed to [onKeep] when Return is pressed or it is left. */
@Composable
private fun KeptField(value: String, placeholder: String, modifier: Modifier = Modifier, onKeep: (String) -> Unit) {
    var text by remember(value) { mutableStateOf(value) }
    var focused by remember { mutableStateOf(false) }
    val line = Hairline
    BasicTextField(
        text,
        onValueChange = { text = it },
        singleLine = true,
        textStyle = TextStyle(color = Foreground, fontSize = 12.sp),
        cursorBrush = SolidColor(Foreground),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onKeep(text.trim()) }),
        decorationBox = { field ->
            Box(Modifier.border(1.dp, line, RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 3.dp)) {
                if (text.isEmpty() && !focused) PaperText(placeholder, 12, Muted, singleLine = true) else field()
            }
        },
        modifier = modifier.onFocusChanged {
            if (focused && !it.isFocused) onKeep(text.trim())
            focused = it.isFocused
        },
    )
}

@Composable
private fun Rule() {
    val line = Hairline
    Canvas(Modifier.fillMaxWidth().height(1.dp)) { drawLine(line, Offset(0f, 0f), Offset(size.width, 0f), 1f) }
}

private val LabCandidate.label: String
    get() = when (this) {
        LabCandidate.REFERENCE -> "Reference"
        LabCandidate.PREVIOUS -> "Previous"
        LabCandidate.WORKING -> "Working"
    }

private val LabLayer.label: String
    get() = when (this) {
        LabLayer.LOW -> "Low"
        LabLayer.BODY -> "Body"
        LabLayer.MID -> "Mid"
        LabLayer.PRESENCE -> "Presence"
        LabLayer.AIR -> "Air"
    }

/** A generator's trim as the candidate bar shows it. */
private fun trimLine(trim: LabTrim): String = when {
    trim.rematchNeeded -> "${decibels(trim.trimDb)} · Trim held · rematch needed"
    trim.matched -> decibels(trim.trimDb)
    else -> "not matched"
}

/** [value] in decibels to a tenth, "−1.8 dB", or to a half for a fader, "−2 dB". */
private fun decibels(value: Float, signed: Boolean = true, whole: Boolean = false): String {
    val tenths = (value * 10).roundToInt()
    val sign = when {
        tenths > 0 && signed -> "+"
        tenths < 0 -> "−"
        else -> ""
    }
    val size = abs(tenths)
    val number = if (whole && size % 10 == 0) "${size / 10}" else "${size / 10}.${size % 10}"
    return "$sign$number dB"
}

/** [seconds] as minutes and seconds, "0:07". */
private fun clock(seconds: Float): String {
    val whole = seconds.coerceAtLeast(0f).toInt()
    return "${whole / 60}:${(whole % 60).toString().padStart(2, '0')}"
}
