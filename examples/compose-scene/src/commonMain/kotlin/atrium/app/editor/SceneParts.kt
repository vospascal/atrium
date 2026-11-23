package atrium.app.editor

import atrium.app.session.Session
import atrium.engine.PlaceChoice
import atrium.engine.SceneKind
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * A part of the editor that some kinds of scene show and others do not. Every part that means
 * something only for some scenes is one of these, and is drawn only where [shows] lists it.
 */
internal enum class ScenePart {
    /** What the scene is, at the top of the World panel, and the switch between kinds. */
    Kind,
    /** How many of the engine's voices the scene uses. */
    Voices,
    /** The place it plays in, Dry or an environment, with its size and whether the sky is open, and the choice of another. */
    Place,
    /** The weather story and its bands, daylight among them. */
    Weather,
    /** The air: temperature and humidity. */
    Air,
    /** Wind, rain, storm and daylight as lanes above the tracks. */
    ConditionLanes,
    /** The weather drawn along the timeline. */
    WeatherOnTimeline,
    /** How fast the weather runs. */
    WeatherSpeed,
    /** Where the wind comes from, drawn on the map. */
    WindOnMap,
    /** How a sound follows the place's conditions, in the Inspector. */
    Follows,
    /** What of the render is heard, and the output and head tracking. */
    Listening,
}

/**
 * What a scene of this kind shows: the one place the kind decides it. A part a kind does not list
 * is not shown, so a part added later stays out of a kind until that kind lists it.
 */
internal fun SceneKind.shows(): Set<ScenePart> = when (this) {
    SceneKind.NATURE -> setOf(
        ScenePart.Kind,
        ScenePart.Voices,
        ScenePart.Place,
        ScenePart.Weather,
        ScenePart.Air,
        ScenePart.ConditionLanes,
        ScenePart.WeatherOnTimeline,
        ScenePart.WeatherSpeed,
        ScenePart.WindOnMap,
        ScenePart.Follows,
        ScenePart.Listening,
    )
    SceneKind.LISTENING -> setOf(
        ScenePart.Kind,
        ScenePart.Voices,
        ScenePart.Place,
        ScenePart.Listening,
    )
}

/** Whether the playing scene shows [part]; with nothing playing, nothing does. */
internal fun Session.shows(part: ScenePart): Boolean = description?.kind?.shows()?.contains(part) == true

/** What a scene kind is called. */
internal val SceneKind.label
    get() = when (this) {
        SceneKind.NATURE -> "Nature"
        SceneKind.LISTENING -> "Listening session"
    }

/**
 * What a place is, in a line under its name: Dry has no room, and an environment its size and whether
 * the sky is open above it ("60 × 60 × 30 m, open sky").
 */
internal fun placeLine(place: PlaceChoice): String {
    if (place.space == null) return "No room: direct sound only"
    val size = listOf(place.width, place.depth, place.height).joinToString(" × ") { metres(it) }
    return "$size m, ${if (place.openSky) "open sky" else "enclosed"}"
}

/** Whole metres as a whole number, else to a tenth. */
private fun metres(value: Float): String {
    val tenths = (value * 10).roundToInt()
    return if (tenths % 10 == 0) "${tenths / 10}" else "${tenths / 10}.${abs(tenths % 10)}"
}

/** What a space is called: the dry space, or the environment file's name ("Meadow"). */
internal fun spaceLabel(space: String?): String =
    space?.substringAfterLast('/')?.substringBeforeLast('.')
        ?.split('_', '-')?.joinToString(" ") { word -> word.replaceFirstChar { it.uppercase() } }
        ?: "Dry"
