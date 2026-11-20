package atrium.app

import java.util.prefs.Preferences

internal actual fun platformSettings(): SettingsStore = object : SettingsStore {
    private val node = Preferences.userRoot().node("atrium/app")

    override fun read(key: String): String? = node.get(key, null)

    override fun write(key: String, value: String) = node.put(key, value)
}
