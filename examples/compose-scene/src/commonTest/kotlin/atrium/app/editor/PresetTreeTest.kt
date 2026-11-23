package atrium.app.editor

import atrium.app.SettingsStore
import atrium.engine.Preset
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class PresetTreeTest {
    private fun preset(path: String, name: String, group: String? = null, about: String? = null) =
        Preset(path, name, group, about, generated = false, takes = 1u)

    /** A slice of the real library, listed in path order as the engine lists it. */
    private val library = listOf(
        preset("sources/creatures/birds/blackbird.yaml", "Blackbird", "Birds"),
        preset("sources/creatures/birds/robin.yaml", "Robin", "Birds", "A robin's song, five takes from one morning."),
        preset("sources/creatures/frog.yaml", "Frog", "Creatures"),
        preset("sources/creatures/cricket.yaml", "Cricket", "Creatures"),
        preset("sources/fire/campfire.yaml", "Campfire", "Fire"),
        preset("sources/music/djembe.yaml", "Djembe", "Music"),
        preset("sources/music/mykonos/bass.yaml", "Mykonos bass", "Mykonos"),
        preset("sources/tones/angel-numbers/1111.yaml", "1111 Hz · Alignment", "Angel numbers"),
        preset("sources/tones/angel-numbers/111.yaml", "111 Hz · New beginnings", "Angel numbers"),
        preset("sources/tones/angel-numbers/222.yaml", "222 Hz · Balance", "Angel numbers"),
        preset("sources/tones/binaural/alpha.yaml", "Alpha Binaural Beat · 10 Hz", "Binaural beats", "108 Hz on the left, 118 Hz on the right."),
        preset("sources/tones/solfeggio/528.yaml", "528 Hz · Transformation", "Solfeggio", "It plays 528, 264, 132 and 66 Hz."),
        preset("sources/tones/solfeggio/174.yaml", "174 Hz · Foundation", "Solfeggio"),
        preset("sources/water/river.yaml", "River", "Water"),
        preset("sources/weather/thunder.yaml", "Thunder", "Weather"),
        preset("sources/weather/wind/soft-wind.yaml", "Soft wind", "Wind"),
    )

    private val tree = presetTree(library, "sources")

    private fun PresetFolder.folder(path: String): PresetFolder =
        if (this.path == path) this else folders.firstNotNullOf { inside -> runCatching { inside.folder(path) }.getOrNull() }

    private fun PresetFolder.names(): List<String> = presets.map { it.name }

    /** Every preset sits in the folder its path names, under folders made from the path. */
    @Test
    fun foldersComeFromThePaths() {
        assertEquals(listOf("creatures", "weather", "water", "fire", "music", "tones"), tree.folders.map { it.path })
        assertEquals(listOf("creatures/birds"), tree.folder("creatures").folders.map { it.path })
        assertEquals(listOf("tones/angel-numbers", "tones/binaural", "tones/solfeggio"), tree.folder("tones").folders.map { it.path })
        assertEquals(listOf("Blackbird", "Robin"), tree.folder("creatures/birds").names())
        assertEquals(listOf("Cricket", "Frog"), tree.folder("creatures").names(), "a folder's own presets, not its sub-folders'")
        assertTrue(tree.folder("tones").presets.isEmpty())
        assertTrue(tree.presets.isEmpty(), "nothing lies in the presets' folder itself")
    }

    /** A preset lying in the presets' folder itself stays at the top, outside every folder. */
    @Test
    fun aPresetAtTheTopStaysThere() {
        val loose = presetTree(listOf(preset("sources/loose.yaml", "Loose"), preset("sources/fire/campfire.yaml", "Campfire")), "sources/")
        assertEquals(listOf("Loose"), loose.names())
        assertEquals(listOf("fire"), loose.folders.map { it.path })
    }

    /** The top folders go from what lives to what is made; a folder not foreseen follows them. */
    @Test
    fun topFoldersInTheirOrder() {
        val withUnknown = presetTree(library + preset("sources/aaa/new.yaml", "New"), "sources")
        assertEquals(listOf("creatures", "weather", "water", "fire", "music", "tones", "aaa"), withUnknown.folders.map { it.path })
    }

    /** Numbers in names go by value, so 111 Hz comes before 222 Hz and 222 Hz before 1111 Hz. */
    @Test
    fun numbersGoByValue() {
        assertEquals(listOf("111 Hz · New beginnings", "222 Hz · Balance", "1111 Hz · Alignment"), tree.folder("tones/angel-numbers").names())
        assertEquals(listOf("174 Hz · Foundation", "528 Hz · Transformation"), tree.folder("tones/solfeggio").names())
        val sorted = listOf("b", "10", "A", "9", "c2", "c10", "007").sortedWith(naturalOrder)
        assertEquals(listOf("007", "9", "10", "A", "b", "c2", "c10"), sorted)
    }

    /** A folder counts its own presets and those of every folder inside it. */
    @Test
    fun countsIncludeSubFolders() {
        assertEquals(library.size, tree.count)
        assertEquals(4, tree.folder("creatures").count)
        assertEquals(2, tree.folder("creatures/birds").count)
        assertEquals(6, tree.folder("tones").count)
        assertEquals(2, tree.folder("music").count)
    }

    @Test
    fun folderTitlesReadAsWords() {
        assertEquals("Angel numbers", folderTitle("angel-numbers"))
        assertEquals("Concert pitch", folderTitle("concert-pitch"))
        assertEquals("Mykonos", folderTitle("mykonos"))
        assertEquals("Angel numbers", tree.folder("tones/angel-numbers").title)
        assertEquals("Birds", tree.folder("creatures/birds").title)
    }

    /** With nothing asked, the whole library shows, with only the remembered folders open. */
    @Test
    fun anEmptySearchShowsEverything() {
        val view = libraryView(tree, "  ", remembered = setOf("weather"))
        assertFalse(view.searching)
        assertEquals(tree, view.shown)
        assertEquals(setOf("weather"), view.open)
    }

    /** A search keeps only the matching presets, and opens exactly the folders holding them. */
    @Test
    fun matchesOpenTheirFolders() {
        val remembered = setOf("weather")
        val view = libraryView(tree, "robin", remembered)
        assertTrue(view.searching)
        assertEquals(1, view.shown.count)
        assertEquals(listOf("creatures"), view.shown.folders.map { it.path })
        assertEquals(setOf("creatures", "creatures/birds"), view.open)
        assertEquals(setOf("weather"), remembered, "a search leaves the remembered folders as they were")
        assertEquals(setOf("weather"), libraryView(tree, "", remembered).open, "and they are back once it is cleared")
    }

    /** A folder folded by hand during a search stays folded until the search changes. */
    @Test
    fun aFolderFoldedWhileSearching() {
        val view = libraryView(tree, "robin", emptySet(), foldedWhileSearching = setOf("creatures/birds"))
        assertEquals(setOf("creatures"), view.open)
    }

    /** Case does not matter, and every word must be found somewhere: name, group, note or folder. */
    @Test
    fun wordsMatchNameGroupNoteAndFolder() {
        fun found(query: String) = libraryView(tree, query, emptySet()).shown.let { shown ->
            fun all(folder: PresetFolder): List<String> = folder.names() + folder.folders.flatMap(::all)
            all(shown).toSet()
        }
        assertEquals(setOf("Robin"), found("ROBIN"))
        assertEquals(setOf("Mykonos bass"), found("mykonos"), "by group")
        assertEquals(setOf("Robin"), found("five takes"), "by words in the note")
        assertEquals(setOf("Blackbird", "Robin"), found("birds"), "by group and folder")
        assertEquals(setOf("111 Hz · New beginnings", "222 Hz · Balance", "1111 Hz · Alignment"), found("angel"))
        assertEquals(setOf("528 Hz · Transformation"), found("solfeggio trans"))
        assertEquals(emptySet(), found("robin thunder"), "every word must match")
        assertEquals(0, libraryView(tree, "nothing like this", emptySet()).shown.count)
        assertTrue(libraryView(tree, "nothing like this", emptySet()).open.isEmpty())
    }

    /** Hz in a name is found with or without the space, and a number matches only where one starts. */
    @Test
    fun hertzInTheName() {
        fun found(query: String) = libraryView(tree, query, emptySet()).shown.let { shown ->
            fun all(folder: PresetFolder): List<String> = folder.names() + folder.folders.flatMap(::all)
            all(shown).toSet()
        }
        assertEquals(setOf("528 Hz · Transformation"), found("528"))
        assertEquals(setOf("528 Hz · Transformation"), found("528hz"))
        assertEquals(setOf("528 Hz · Transformation"), found("528 HZ"))
        assertEquals(setOf("111 Hz · New beginnings", "1111 Hz · Alignment"), found("111"), "typing on narrows it")
        assertEquals(setOf("1111 Hz · Alignment"), found("1111 hz"))
        assertEquals(setOf("Alpha Binaural Beat · 10 Hz"), found("10 hz"), "10 Hz is not 110 Hz or 1111 Hz")
        assertEquals(setOf("Alpha Binaural Beat · 10 Hz"), found("118"), "a number in the note")
    }
}

class LibraryFoldsTest {
    private class Memory : SettingsStore {
        val values = mutableMapOf<String, String>()
        override fun read(key: String) = values[key]
        override fun write(key: String, value: String) {
            values[key] = value
        }
    }

    /** Every folder starts folded; one opened is open again after a restart, and folds again. */
    @Test
    fun openFoldersAreKept() {
        val store = Memory()
        assertTrue(Workspace(store).libraryOpen.isEmpty())
        Workspace(store).toggleLibraryFolder("creatures")
        Workspace(store).toggleLibraryFolder("tones/angel-numbers")
        assertEquals(setOf("creatures", "tones/angel-numbers"), Workspace(store).libraryOpen)
        Workspace(store).toggleLibraryFolder("creatures")
        assertEquals(setOf("tones/angel-numbers"), Workspace(store).libraryOpen)
    }
}
