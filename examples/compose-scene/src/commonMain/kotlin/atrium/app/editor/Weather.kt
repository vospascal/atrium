package atrium.app.editor

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.drawText
import atrium.engine.Band
import atrium.engine.Conditions
import atrium.engine.Follow
import atrium.engine.Story
import atrium.engine.Weather
import kotlinx.coroutines.delay
import kotlin.math.roundToInt
import atrium.app.ui.Chip
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.session.Session
import atrium.app.ui.labelsThatFit

/** A condition of the place, as the app names and reads it. */
internal enum class Condition(val label: String) {
    Wind("Wind"),
    Rain("Rain"),
    Storm("Storm"),
    Daylight("Daylight"),
    ;

    fun of(conditions: Conditions): Float = when (this) {
        Wind -> conditions.wind
        Rain -> conditions.rain
        Storm -> conditions.storm
        Daylight -> conditions.daylight
    }

    fun of(weather: Weather): Band = when (this) {
        Wind -> weather.wind
        Rain -> weather.rain
        Storm -> weather.storm
        Daylight -> weather.daylight
    }

    fun with(weather: Weather, band: Band): Weather = when (this) {
        Wind -> weather.copy(wind = band)
        Rain -> weather.copy(rain = band)
        Storm -> weather.copy(storm = band)
        Daylight -> weather.copy(daylight = band)
    }

    companion object {
        /** The condition a file names, if there is one. */
        fun named(name: String) = entries.firstOrNull { it.name.lowercase() == name }
    }
}

internal val Story.label
    get() = when (this) {
        Story.HELD -> "Held"
        Story.DRIFTING -> "Drifting"
        Story.PASSING_STORM -> "Passing storm"
    }

/**
 * What moves the place: the weather story and the band each condition keeps
 * to, with where each is now. Every sound that follows a condition follows
 * these; a passing storm lifts wind, rain and storm above their bands.
 */
@Composable
internal fun WeatherPart(session: Session) {
    val weather = session.weather ?: return
    val now by produceState(emptyList<Conditions>(), session.engine) {
        while (true) {
            value = session.conditionsAround(0f, 0f, 1)
            delay(250)
        }
    }
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Story.entries.forEach { story ->
            Chip(story.label, selected = story == weather.story) { session.setWeather(weather.copy(story = story)) }
        }
    }
    Spacer(Modifier.height(8.dp))
    PaperText(storyLine(weather.story), 14, Muted)
    Spacer(Modifier.height(6.dp))
    for (condition in Condition.entries) {
        val band = condition.of(weather)
        val current = now.firstOrNull()?.let(condition::of)
        Spacer(Modifier.height(8.dp))
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            PaperText(condition.label, 15, modifier = Modifier.weight(1f))
            PaperText(bandLabel(band, weather.story), 13, Muted)
            if (current != null) {
                Spacer(Modifier.width(10.dp))
                PaperText("now ${percent(current)}", 13)
            }
        }
        TwoHandleRange(
            band.low, band.high,
            toTrack = { it },
            fromTrack = { (it * 100f).roundToInt() / 100f },
            marks = ConditionMarks,
            step = .01f,
            onChange = { low, high -> session.setWeather(condition.with(weather, Band(low, high))) },
        )
    }
}

private val ConditionMarks = listOf(0f to "0", .5f to "half", 1f to "full")

private fun storyLine(story: Story) = when (story) {
    Story.HELD -> "Each condition stays in the middle of its band."
    Story.DRIFTING -> "Each condition wanders slowly inside its band."
    Story.PASSING_STORM -> "They drift, and now and then a storm passes: gusts, a downpour with thunder, then dripping."
}

private fun bandLabel(band: Band, story: Story): String = when {
    story == Story.HELD || band.low == band.high -> percent((band.low + band.high) / 2f)
    else -> "${percent(band.low)}–${percent(band.high)}"
}

private fun percent(value: Float) = "${(value * 100).roundToInt()} %"

/** What a sound does as the conditions change, one line per link. */
@Composable
internal fun FollowsPart(follows: List<Follow>) {
    if (follows.isEmpty()) {
        PaperText("Nothing: the weather does not change it", 15)
        return
    }
    for (follow in follows) PaperText(followLine(follow), 15)
}

/** A link in words: "Activity 100 % → 0 % as rain rises". */
internal fun followLine(follow: Follow): String {
    val setting = follow.setting.replace('_', ' ').replaceFirstChar { it.uppercase() }
    val condition = Condition.named(follow.condition)?.label?.lowercase() ?: follow.condition
    val value = { amount: Float ->
        when (follow.setting) {
            "activity" -> percent(amount)
            "level" -> "${amount.roundToInt()} dB"
            else -> if (amount == amount.roundToInt().toFloat()) "${amount.roundToInt()}" else "${(amount * 100).roundToInt() / 100f}"
        }
    }
    val over = follow.over
    val stretch = if (over.low <= 0f && over.high >= 1f) "as $condition rises" else "as $condition goes from ${percent(over.low)} to ${percent(over.high)}"
    return "$setting ${value(follow.from)} → ${value(follow.to)} $stretch"
}

/** How much of the scene the overview shows at once (s). */
private const val OverviewWindow = 3 * 3600f

/** The weather speeds a person can choose: as it is, ten times, and an hour a minute. */
private val WeatherSpeeds = listOf(1f to "1×", 10f to "10×", 60f to "60×")

/**
 * The whole scene as a timeline, like the scrub bar of a video editor: three
 * hours of it with the weather drawn in, rain filled, the storm darker over
 * it and the wind as a line, and the playhead where the scene is [now]. Press
 * or drag anywhere to move the whole scene there, every sound and the
 * weather. The speed runs the weather faster while the sounds keep their own
 * pace.
 */
@Composable
internal fun Overview(session: Session, now: Double, modifier: Modifier = Modifier) {
    // The three hours the playhead is in.
    val windowStart = ((now / OverviewWindow).toInt() * OverviewWindow).toDouble()
    val weather by produceState(emptyList<Conditions>(), session.engine, session.weather, session.weatherSpeed, windowStart) {
        while (true) {
            val at = session.sceneTime()
            value = session.conditionsAround((windowStart - at).toFloat(), (windowStart + OverviewWindow - at).toFloat(), 360)
            delay(1000)
        }
    }
    val ink = Foreground
    val faint = Hairline
    val text = androidx.compose.ui.text.rememberTextMeasurer()
    val muted = Muted
    val seek = androidx.compose.runtime.rememberUpdatedState { fraction: Float ->
        session.seek(windowStart + fraction.coerceIn(0f, 1f) * OverviewWindow)
    }
    Row(modifier, verticalAlignment = Alignment.CenterVertically) {
        PaperText("TIMELINE", 12, Muted, androidx.compose.ui.text.font.FontWeight.SemiBold, spacing = 2f, modifier = Modifier.width(100.dp))
        androidx.compose.foundation.Canvas(
            Modifier.weight(1f).height(40.dp)
                .pointerHoverIcon(androidx.compose.ui.input.pointer.PointerIcon.Hand)
                .pointerInput(Unit) { detectTapGestures { seek.value(it.x / size.width) } }
                .pointerInput(Unit) { detectDragGestures { change, _ -> seek.value(change.position.x / size.width) } },
        ) {
            val lane = size.height - 12.dp.toPx()
            drawRect(faint.copy(alpha = .35f), size = androidx.compose.ui.geometry.Size(size.width, lane))
            if (weather.size > 1) {
                fun curve(of: (Conditions) -> Float, close: Boolean) = androidx.compose.ui.graphics.Path().apply {
                    if (close) moveTo(0f, lane)
                    weather.forEachIndexed { index, conditions ->
                        val x = size.width * index / (weather.size - 1)
                        val y = lane - lane * of(conditions)
                        if (index == 0 && !close) moveTo(x, y) else lineTo(x, y)
                    }
                    if (close) {
                        lineTo(size.width, lane)
                        close()
                    }
                }
                drawPath(curve({ it.rain }, true), ink.copy(alpha = .22f))
                drawPath(curve({ it.storm }, true), ink.copy(alpha = .35f))
                drawPath(curve({ it.wind }, false), ink.copy(alpha = .6f), style = androidx.compose.ui.graphics.drawscope.Stroke(1.dp.toPx()))
            }
            val hours = (0..3).map { hour ->
                val x = size.width * hour / 3f
                val label = text.measure("${(windowStart / 3600).toInt() + hour}:00", androidx.compose.ui.text.TextStyle(fontSize = 10.sp, color = muted))
                label to (x - label.size.width / 2f).coerceIn(0f, size.width - label.size.width)
            }
            // The first and last hour first: in a narrow panel the ones between give way.
            val kept = labelsThatFit(hours.map { (label, at) -> at..(at + label.size.width) }, 6.dp.toPx(), listOf(0, 3, 1, 2))
            hours.forEachIndexed { index, (label, at) ->
                if (index in kept) drawText(label, topLeft = Offset(at, lane + 1.dp.toPx()))
            }
            val x = size.width * ((now - windowStart) / OverviewWindow).toFloat().coerceIn(0f, 1f)
            drawLine(ink, Offset(x, 0f), Offset(x, lane), 2.dp.toPx())
            drawCircle(ink, 4.dp.toPx(), Offset(x, 0f))
        }
        Spacer(Modifier.width(10.dp))
        WeatherSpeeds.forEach { (speed, label) ->
            PaperText(
                label,
                12,
                if (session.weatherSpeed == speed) Foreground else Muted,
                if (session.weatherSpeed == speed) androidx.compose.ui.text.font.FontWeight.Bold else androidx.compose.ui.text.font.FontWeight.Normal,
                modifier = Modifier.clickable(role = androidx.compose.ui.semantics.Role.Tab) { session.runWeatherAt(speed) }
                    .padding(horizontal = 5.dp, vertical = 2.dp),
            )
        }
    }
}
