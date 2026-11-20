package atrium.app

import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import java.awt.Dimension

/** Relative to the working directory, which the Gradle run task sets to the repository root. */
private const val DefaultSceneDirectory = "scenes"

fun main(args: Array<String>) = application {
    Window(
        onCloseRequest = ::exitApplication,
        title = "Atrium",
        state = rememberWindowState(width = 440.dp, height = 900.dp),
    ) {
        LaunchedEffect(window) { window.minimumSize = Dimension(360, 640) }
        AtriumApp(args.firstOrNull() ?: DefaultSceneDirectory)
    }
}
