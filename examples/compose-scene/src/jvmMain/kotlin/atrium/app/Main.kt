package atrium.app

import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import java.awt.Dimension

/** Relative to the working directory, which the Gradle run task sets to the repository root. */
private const val DefaultSceneDirectory = "scenes"

/** The player is a phone-shaped column; the editor needs room for three panels. */
private val PlayerWindow = DpSize(440.dp, 900.dp)
private val EditorWindow = DpSize(1240.dp, 900.dp)

fun main(args: Array<String>) = application {
    val state = rememberWindowState(size = PlayerWindow)
    Window(onCloseRequest = ::exitApplication, title = "Atrium", state = state) {
        LaunchedEffect(window) { window.minimumSize = Dimension(360, 640) }
        AtriumApp(
            args.firstOrNull() ?: DefaultSceneDirectory,
            onEditing = { editing -> state.size = if (editing) EditorWindow else PlayerWindow },
        )
    }
}
