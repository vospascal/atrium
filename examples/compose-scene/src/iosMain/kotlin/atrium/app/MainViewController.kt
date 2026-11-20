package atrium.app

import androidx.compose.ui.window.ComposeUIViewController
import platform.UIKit.UIViewController

/**
 * The app for an iOS host. [sceneDirectory] and every path inside a scene
 * file resolve against the process working directory.
 */
fun MainViewController(sceneDirectory: String): UIViewController =
    ComposeUIViewController { AtriumApp(sceneDirectory) }
