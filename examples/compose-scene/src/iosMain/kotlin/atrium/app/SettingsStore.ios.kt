package atrium.app

import platform.Foundation.NSUserDefaults

internal actual fun platformSettings(): SettingsStore = object : SettingsStore {
    private val defaults = NSUserDefaults.standardUserDefaults

    override fun read(key: String): String? = defaults.stringForKey(key)

    override fun write(key: String, value: String) = defaults.setObject(value, forKey = key)
}
