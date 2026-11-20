package atrium.app

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue

/** Inspect a sound in the playing scene and drag it if it is localized. */
@Composable
internal fun Composer(session: Session, onBack: () -> Unit) {
    val description = session.description
    val sourceCount = description?.sources?.size ?: 0
    var selected by remember(description) { mutableStateOf(0) }
    val slot = selected.toUShort()
    var scene by remember(description, selected) { mutableStateOf(description?.toScene(selected) ?: Scene.Preview) }
    val drag = remember(description, selected) { DragEcho() }

    val telemetry = session.telemetry
    LaunchedEffect(telemetry, selected) {
        val source = telemetry?.sources?.firstOrNull { it.slot == slot } ?: return@LaunchedEffect
        val reported = source.position.toPoint()
        scene = scene.copy(
            emitter = if (drag.accepts(reported)) reported else scene.emitter,
            facing = source.facing.toPoint(),
        )
    }

    key(selected) {
        SceneEditor(
            scene = scene,
            status = session.status,
            running = session.engine != null,
            peakDb = telemetry?.peaksDb?.maxOrNull(),
            onBack = onBack,
            onStepSource = if (sourceCount > 1) {
                { by -> selected = (selected + by).mod(sourceCount) }
            } else {
                null
            },
            onDrag = { position ->
                scene = scene.copy(emitter = position)
                drag.move()
                session.command { setSourcePosition(slot, position.toPosition()) }
            },
            onDragFinished = {
                drag.release(scene.emitter)
                session.command { setSourcePosition(slot, scene.emitter.toPosition()) }
            },
        )
    }
}
