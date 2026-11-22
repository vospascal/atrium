package atrium.app.editor

/**
 * Ask for a recording to open as the Lab's reference, the way the platform asks for a file: its
 * path, relative to the working directory when it lies inside it, or null when nothing was chosen
 * or the platform has no way to ask.
 */
internal expect fun chooseRecording(): String?
