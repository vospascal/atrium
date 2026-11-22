package atrium.app.editor

import java.awt.FileDialog
import java.awt.Frame
import java.io.File

/** The audio files the engine decodes. */
private val RecordingExtensions = setOf("wav", "mp3", "flac", "ogg", "m4a", "aac")

/** The macOS file dialog, opening on the repository's reference recordings when they are there. */
internal actual fun chooseRecording(): String? {
    val dialog = FileDialog(null as Frame?, "Open recording", FileDialog.LOAD)
    val workingDirectory = File(".").canonicalFile
    val references = listOf("assets/reference", "research papers")
        .map { workingDirectory.resolve(it) }
        .firstOrNull { it.isDirectory }
    dialog.directory = (references ?: workingDirectory).path
    dialog.setFilenameFilter { _, name -> name.substringAfterLast('.').lowercase() in RecordingExtensions }
    dialog.isVisible = true
    val name = dialog.file ?: return null
    val chosen = File(dialog.directory, name).canonicalFile
    val inside = chosen.path.startsWith(workingDirectory.path + File.separator)
    return if (inside) chosen.relativeTo(workingDirectory).path else chosen.path
}
