package atrium.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import atrium.app.ui.Foreground
import atrium.app.ui.Hairline
import atrium.app.ui.Muted
import atrium.app.ui.PaperText
import atrium.app.session.Session

/**
 * A small meter for each output channel: its peak in the last buffer, from −60 dBFS to full
 * scale, named as the room names its loudspeaker or L and R on headphones. A channel at full
 * scale is clipping, and its name turns bold.
 */
@Composable
internal fun ChannelMeters(session: Session) {
    val telemetry = session.telemetry ?: return
    val names = channelNames(telemetry.peaksDb.size, telemetry.channelsInUse.isEmpty(), session.description?.speakers.orEmpty().associate { it.channel.toInt() to it.name })
    val track = Hairline
    val fill = Foreground
    Row(verticalAlignment = Alignment.CenterVertically) {
        telemetry.peaksDb.forEachIndexed { channel, peak ->
            val clipping = peak >= ClippingFrom
            PaperText(names[channel], 11, if (clipping) Foreground else Muted, if (clipping) FontWeight.Bold else FontWeight.Normal, singleLine = true)
            Canvas(Modifier.padding(start = 4.dp, end = 10.dp).width(30.dp).height(8.dp)) {
                val middle = size.height / 2
                drawLine(track, Offset(0f, middle), Offset(size.width, middle), strokeWidth = 3.dp.toPx())
                val level = meterOf(peak)
                if (level > 0f) drawLine(fill, Offset(0f, middle), Offset(size.width * level, middle), strokeWidth = (if (clipping) 5.dp else 3.dp).toPx())
            }
        }
    }
}

/**
 * What each of [count] output channels is called: L and R on [headphones], else the room's own
 * name for the loudspeaker on that channel from [speakers], and LFE or the channel's number for
 * one no loudspeaker stands on.
 */
internal fun channelNames(count: Int, headphones: Boolean, speakers: Map<Int, String>): List<String> = List(count) { channel ->
    when {
        headphones && count == 2 -> if (channel == 0) "L" else "R"
        channel in speakers -> speakers.getValue(channel)
        count == 6 && channel == 3 -> "LFE"
        else -> "${channel + 1}"
    }
}

/** A peak this close to full scale is clipping (dBFS). */
private const val ClippingFrom = -.1f

/** How full a speaker's meter is for a peak of [peakDb] dBFS: empty at −60, full at 0. */
internal fun meterOf(peakDb: Float): Float = ((peakDb - QuietestMeter) / -QuietestMeter).coerceIn(0f, 1f)

private const val QuietestMeter = -60f
