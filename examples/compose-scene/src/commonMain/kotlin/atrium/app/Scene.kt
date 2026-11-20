package atrium.app

import atrium.engine.EmitterKind
import atrium.engine.Position
import atrium.engine.SceneDescription
import kotlin.math.abs
import kotlin.math.hypot

internal data class Point(val x: Float, val y: Float, val z: Float = 0f) {
    fun groundDistanceTo(other: Point) = hypot(x - other.x, y - other.y)
    fun hasGroundBearing(): Boolean = abs(x) >= 1e-4f || abs(y) >= 1e-4f
}

internal data class Scene(
    val width: Float,
    val depth: Float,
    val listener: Point,
    val emitter: Point,
    val emitterName: String,
    val levelAtReference: Float,
    val referenceDistance: Float,
    val emitterKind: EmitterKind = EmitterKind.OBJECT,
    val beamHalfAngle: Float? = null,
    val facing: Point = Point(1f, 0f),
) {
    val isField: Boolean get() = emitterKind == EmitterKind.FIELD
    val offset: Point get() = Point(emitter.x - listener.x, emitter.y - listener.y)

    companion object {
        /** Shown until an engine is running, and when none can start. */
        val Preview = Scene(60f, 60f, Point(30f, 30f, 1.2f), Point(40.1f, 40.1f), "Frogs", 65f, 1f)
    }
}

internal fun SceneDescription.toScene(slot: Int): Scene? {
    val emitter = sources.getOrNull(slot) ?: return null
    return Scene(
        width = width,
        depth = depth,
        listener = listener.toPoint(),
        emitter = emitter.position.toPoint(),
        emitterName = emitter.name,
        levelAtReference = emitter.spl,
        referenceDistance = emitter.referenceDistance,
        emitterKind = emitter.emitterKind,
        beamHalfAngle = emitter.beamHalfAngle,
        facing = emitter.facing.toPoint(),
    )
}

internal fun Position.toPoint() = Point(x, y, z)

internal fun Point.toPosition() = Position(x, y, z)
