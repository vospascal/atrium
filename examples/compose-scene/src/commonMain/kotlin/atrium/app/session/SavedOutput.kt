package atrium.app.session

import atrium.engine.ChannelMode
import atrium.engine.Output
import atrium.engine.RenderMode

/** An output as the settings store keeps it, such as `HRTF/STEREO`. */
internal fun Output.saved(): String = "${renderMode.name}/${channelMode.name}"

/** The output [saved] names, or null when nothing was saved or it names an output this version does not have. */
internal fun outputSaved(saved: String?): Output? {
    val names = saved?.split('/')?.takeIf { it.size == 2 } ?: return null
    val renderMode = RenderMode.entries.firstOrNull { it.name == names[0] } ?: return null
    val channelMode = ChannelMode.entries.firstOrNull { it.name == names[1] } ?: return null
    return Output(renderMode, channelMode)
}
