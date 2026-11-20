package atrium.app

/** Small text settings that outlive the app: preferences on desktop, user defaults on iOS. */
internal interface SettingsStore {
    fun read(key: String): String?
    fun write(key: String, value: String)
}

/** This platform's store. */
internal expect fun platformSettings(): SettingsStore
