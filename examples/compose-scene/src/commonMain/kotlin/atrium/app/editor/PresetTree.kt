package atrium.app.editor

import atrium.engine.Preset

/**
 * A folder of the library: the presets lying directly in it and the folders inside it, as
 * they stand under the presets' folder on disk. [path] is the folder's path under that folder,
 * "creatures/birds", and "" for the presets' folder itself. A folder lists its own presets
 * before its sub-folders, each in [presetOrder] and [folderOrder].
 */
internal data class PresetFolder(val path: String, val presets: List<Preset>, val folders: List<PresetFolder>) {
    /** What the library calls it: "angel-numbers" is "Angel numbers". */
    val title: String get() = folderTitle(path.substringAfterLast('/'))

    /** How many presets are in it, those in its sub-folders included. */
    val count: Int = presets.size + folders.sumOf { it.count }

    /** The paths of every folder inside it, however deep. */
    fun folderPaths(): Set<String> = folders.flatMap { it.folderPaths() + it.path }.toSet()
}

/**
 * The library as one screen shows it: the folders to [shown], which of them are [open], and
 * whether that is a search. While searching, only the presets that match and the folders that
 * hold them are shown, and those folders are open whatever was remembered.
 */
internal data class LibraryView(val shown: PresetFolder, val open: Set<String>, val searching: Boolean)

/**
 * The library for [query]: the whole [tree] with the folders in [remembered] open, or, while
 * [query] asks for something, the presets matching it with every folder holding one open,
 * except those folded by hand during that search ([foldedWhileSearching]). [remembered] is
 * only read, so a search never changes what opens after it.
 */
internal fun libraryView(
    tree: PresetFolder,
    query: String,
    remembered: Set<String>,
    foldedWhileSearching: Set<String> = emptySet(),
): LibraryView {
    val words = searchWords(query)
    if (words.isEmpty()) return LibraryView(tree, remembered, searching = false)
    val shown = tree.matching(words)
    return LibraryView(shown, shown.folderPaths() - foldedWhileSearching, searching = true)
}

/**
 * The folders the [presets] lie in under [directory], the folder they were listed from, as a
 * tree. A preset whose path does not lie under [directory] is put in the top folder.
 */
internal fun presetTree(presets: List<Preset>, directory: String): PresetFolder {
    val byFolder = presets.groupBy { folderOf(it.path, directory) }
    val everyFolder = byFolder.keys.flatMap(::withParents).toSet()
    fun build(path: String): PresetFolder {
        val inside = everyFolder.filter { it.isNotEmpty() && parentOf(it) == path }
        val order = if (path.isEmpty()) TopFolderOrder else folderOrder
        return PresetFolder(path, byFolder[path].orEmpty().sortedWith(presetOrder), inside.sortedWith(order).map(::build))
    }
    return build("")
}

/** "angel-numbers" as a title: "Angel numbers". */
internal fun folderTitle(name: String): String =
    name.replace('-', ' ').replace('_', ' ').trim().replaceFirstChar { it.uppercaseChar() }

/**
 * Names in the order a person reads them: numbers by their value, so 111 comes before 222 and
 * 222 before 1111, numbers before words, and words alphabetically without regard to case.
 */
internal val naturalOrder: Comparator<String> = Comparator { first, second ->
    val firstParts = naturalParts(first)
    val secondParts = naturalParts(second)
    for (index in 0 until minOf(firstParts.size, secondParts.size)) {
        val compared = compareParts(firstParts[index], secondParts[index])
        if (compared != 0) return@Comparator compared
    }
    firstParts.size.compareTo(secondParts.size)
}

/** Presets in a folder: by name in [naturalOrder], and by path where two share a name. */
private val presetOrder: Comparator<Preset> = Comparator<Preset> { first, second -> naturalOrder.compare(first.name, second.name) }
    .thenBy { it.path }

/** Sub-folders by their names in [naturalOrder]. */
private val folderOrder: Comparator<String> = Comparator { first, second ->
    naturalOrder.compare(first.substringAfterLast('/'), second.substringAfterLast('/'))
}

/**
 * The top folders from what lives to what is made: creatures and the weather, water and fire,
 * then music and tones. A folder not named here follows them, in [folderOrder].
 */
private val TopFolders = listOf("creatures", "weather", "water", "fire", "music", "tones")

private val TopFolderOrder: Comparator<String> = compareBy<String> { folder ->
    TopFolders.indexOf(folder).let { if (it < 0) TopFolders.size else it }
}.then(folderOrder)

/** The words of [query], as the search compares them; none when it asks for nothing. */
private fun searchWords(query: String): List<String> =
    searchable(query).split(' ').filter { it.isNotEmpty() }

/**
 * Whether [preset], lying in the folder at [folderPath], has every one of [words] in its name,
 * its group, its note or its folders' titles. A word starting with a digit matches only where a
 * number starts, so "528" finds 528 Hz and "111" finds 111 and 1111 Hz, but "10 Hz" does not
 * find 110 Hz. "528hz" and "528 Hz" are the same.
 */
private fun presetMatches(preset: Preset, folderPath: String, words: List<String>): Boolean {
    val folders = folderPath.split('/').filter { it.isNotEmpty() }.map(::folderTitle)
    val text = searchable(listOfNotNull(preset.name, preset.group, preset.about, *folders.toTypedArray()).joinToString(" "))
    return words.all { word -> containsWord(text, word) }
}

private fun PresetFolder.matching(words: List<String>): PresetFolder = copy(
    presets = presets.filter { presetMatches(it, path, words) },
    folders = folders.map { it.matching(words) }.filter { it.count > 0 },
)

private fun containsWord(text: String, word: String): Boolean {
    if (!word.first().isDigit()) return word in text
    var from = text.indexOf(word)
    while (from >= 0) {
        val before = text.getOrNull(from - 1)
        if (before == null || !(before.isDigit() || before == '.')) return true
        from = text.indexOf(word, from + 1)
    }
    return false
}

/** [text] lower-case, with its spaces collapsed and "528 Hz" written "528hz". */
private fun searchable(text: String): String =
    text.lowercase().replace(Whitespace, " ").replace(NumberThenHertz, "$1hz").trim()

private val Whitespace = Regex("\\s+")
private val NumberThenHertz = Regex("(\\d) hz")

/** The folder [path] lies in under [directory]: "sources/creatures/birds/robin.yaml" lies in "creatures/birds". */
private fun folderOf(path: String, directory: String): String {
    val file = path.replace('\\', '/')
    val top = directory.replace('\\', '/').trimEnd('/')
    val under = when {
        file.startsWith("$top/") -> file.removePrefix("$top/")
        file.contains("/$top/") -> file.substringAfter("/$top/")
        else -> return ""
    }
    return under.substringBeforeLast('/', missingDelimiterValue = "")
}

private fun parentOf(path: String): String = path.substringBeforeLast('/', missingDelimiterValue = "")

/** "a/b" and the folders it lies in: "", "a" and "a/b". */
private fun withParents(path: String): List<String> {
    if (path.isEmpty()) return listOf("")
    val names = path.split('/')
    return listOf("") + names.indices.map { names.take(it + 1).joinToString("/") }
}

/** A name cut into runs of digits and runs of anything else. */
private fun naturalParts(name: String): List<String> {
    val parts = mutableListOf<String>()
    var start = 0
    for (index in 1..name.length) {
        if (index == name.length || name[index].isDigit() != name[start].isDigit()) {
            parts += name.substring(start, index)
            start = index
        }
    }
    return parts
}

private fun compareParts(first: String, second: String): Int {
    val firstNumber = first.first().isDigit()
    val secondNumber = second.first().isDigit()
    return when {
        firstNumber && secondNumber -> {
            // By value, without parsing: fewer digits is smaller once leading zeros are gone.
            val firstDigits = first.trimStart('0')
            val secondDigits = second.trimStart('0')
            if (firstDigits.length != secondDigits.length) firstDigits.length.compareTo(secondDigits.length) else firstDigits.compareTo(secondDigits)
        }
        firstNumber -> -1
        secondNumber -> 1
        else -> first.compareTo(second, ignoreCase = true)
    }
}
