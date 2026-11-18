import { useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import type { SceneSource, SceneState, SourceTelemetry } from '../protocol'

interface SceneMapProps {
  scene: SceneState
  live: SourceTelemetry[]
  /** Called while a sound is dragged, with the new centre in world metres. */
  onMove: (index: number, x: number, y: number, z: number) => void
}

interface Bounds {
  minX: number
  maxX: number
  minY: number
  maxY: number
}

/** A drag in progress: which sound, and where its centre is being put. */
interface Drag {
  index: number
  /** Pointer position in world metres when the drag began. */
  pointerStart: { x: number; y: number }
  /** The sound's centre when the drag began. */
  centreStart: { x: number; y: number; z: number }
  /** The centre last sent to the engine. */
  centre: { x: number; y: number }
  /** The view as it was when the drag began, so the map does not zoom under the finger. */
  bounds: Bounds
}

/** A wisp flies its own path; the engine ignores a position set from outside. */
function canBeMoved(source: SceneSource): boolean {
  return source.emitter_kind !== 'field' && source.synth_kind !== 'wisp'
}

/** Smallest square view that shows the room, every speaker and every sound. */
function viewBounds(scene: SceneState, live: SourceTelemetry[]): Bounds {
  const halfWidth = scene.atrium.width / 2
  const halfDepth = scene.atrium.depth / 2
  const xs = [scene.spawn.x - halfWidth, scene.spawn.x + halfWidth, scene.listener.x]
  const ys = [scene.spawn.y - halfDepth, scene.spawn.y + halfDepth, scene.listener.y]
  for (const speaker of scene.speakers) {
    xs.push(speaker.x)
    ys.push(speaker.y)
  }
  scene.sources.forEach((source, index) => {
    if (source.emitter_kind === 'field') return
    const state = live[index]
    const centreX = state?.ocx ?? source.position[0]
    const centreY = state?.ocy ?? source.position[1]
    const reach = Math.abs(state?.or ?? source.orbit_radius)
    xs.push(centreX - reach, centreX + reach)
    ys.push(centreY - reach, centreY + reach)
    if (state) {
      xs.push(state.x)
      ys.push(state.y)
    }
  })
  const centreX = (Math.min(...xs) + Math.max(...xs)) / 2
  const centreY = (Math.min(...ys) + Math.max(...ys)) / 2
  const half = Math.max(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys)) / 2
  const padded = half * 1.15 + 0.5
  return {
    minX: centreX - padded,
    maxX: centreX + padded,
    minY: centreY - padded,
    maxY: centreY + padded,
  }
}

/**
 * The scene from above. World +Y (the front of the room) points up, so every
 * y is negated on the way into SVG, whose y axis points down. Sounds can be
 * dragged; an orbiting sound moves as a whole, orbit and all.
 */
export function SceneMap({ scene, live, onMove }: SceneMapProps) {
  const svgRef = useRef<SVGSVGElement>(null)
  const [drag, setDrag] = useState<Drag | null>(null)

  const bounds = drag?.bounds ?? viewBounds(scene, live)
  const size = bounds.maxX - bounds.minX
  const unit = size / 100 // one hundredth of the view: keeps marks the same size at any zoom
  const forward = { x: Math.cos(scene.listener.yaw), y: Math.sin(scene.listener.yaw) }

  /** Where the pointer is, in world metres. */
  const pointerInWorld = (event: ReactPointerEvent): { x: number; y: number } | null => {
    const svg = svgRef.current
    const matrix = svg?.getScreenCTM()
    if (!svg || !matrix) return null
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse())
    return { x: point.x, y: -point.y }
  }

  const beginDrag = (event: ReactPointerEvent<SVGCircleElement>, index: number) => {
    const pointer = pointerInWorld(event)
    if (!pointer) return
    const source = scene.sources[index]
    const state = live[index]
    event.currentTarget.setPointerCapture(event.pointerId)
    const centreStart = {
      x: state?.ocx ?? source.position[0],
      y: state?.ocy ?? source.position[1],
      z: state?.z ?? source.position[2],
    }
    setDrag({ index, pointerStart: pointer, centreStart, centre: centreStart, bounds })
  }

  const continueDrag = (event: ReactPointerEvent<SVGCircleElement>) => {
    if (!drag) return
    const pointer = pointerInWorld(event)
    if (!pointer) return
    const centre = {
      x: drag.centreStart.x + (pointer.x - drag.pointerStart.x),
      y: drag.centreStart.y + (pointer.y - drag.pointerStart.y),
    }
    setDrag({ ...drag, centre })
    onMove(drag.index, centre.x, centre.y, drag.centreStart.z)
  }

  return (
    <section className="card map-card">
      <h2>
        Scene
        <span className="value">{size.toFixed(0)} m across</span>
      </h2>
      <svg
        ref={svgRef}
        className="map"
        viewBox={`${bounds.minX} ${-bounds.maxY} ${size} ${size}`}
        role="img"
        aria-label="Top-down view of the scene: room, speakers, listener and sounds"
      >
        <rect
          className="map-environment"
          x={0}
          y={-scene.room.depth}
          width={scene.room.width}
          height={scene.room.depth}
          strokeWidth={unit * 0.3}
        />
        <rect
          className="map-room"
          x={scene.spawn.x - scene.atrium.width / 2}
          y={-(scene.spawn.y + scene.atrium.depth / 2)}
          width={scene.atrium.width}
          height={scene.atrium.depth}
          strokeWidth={unit * 0.4}
        />

        {scene.speakers.map((speaker) => (
          <g key={speaker.channel}>
            <rect
              className="map-speaker"
              x={speaker.x - unit * 1.4}
              y={-speaker.y - unit * 1.4}
              width={unit * 2.8}
              height={unit * 2.8}
              rx={unit * 0.5}
            />
            <text className="map-label" x={speaker.x} y={-speaker.y - unit * 2.4} fontSize={unit * 3} textAnchor="middle">
              {speaker.label}
            </text>
          </g>
        ))}

        <line
          className="map-facing"
          x1={scene.listener.x}
          y1={-scene.listener.y}
          x2={scene.listener.x + forward.x * unit * 6}
          y2={-(scene.listener.y + forward.y * unit * 6)}
          strokeWidth={unit * 0.7}
        />
        <circle className="map-listener" cx={scene.listener.x} cy={-scene.listener.y} r={unit * 2} />

        {scene.sources.map((source, index) => {
          if (source.emitter_kind === 'field') return null
          const state = live[index]
          const dragged = drag?.index === index ? drag : null
          // The engine reports about 15 times a second. While dragging, show
          // the sound where it is being put: its reported position shifted by
          // however far the centre has moved since that report.
          const reportedCentre = {
            x: state?.ocx ?? source.position[0],
            y: state?.ocy ?? source.position[1],
          }
          const centre = dragged?.centre ?? reportedCentre
          const x = (state?.x ?? source.position[0]) + (centre.x - reportedCentre.x)
          const y = (state?.y ?? source.position[1]) + (centre.y - reportedCentre.y)
          const orbitRadius = Math.abs(state?.or ?? source.orbit_radius)
          const movable = canBeMoved(source)
          return (
            <g key={`${source.name}-${index}`} opacity={state?.muted ? 0.3 : 1}>
              {orbitRadius > 0.01 && (
                <circle
                  className="map-orbit"
                  cx={centre.x}
                  cy={-centre.y}
                  r={orbitRadius}
                  stroke={source.color}
                  strokeWidth={unit * 0.3}
                />
              )}
              <circle cx={x} cy={-y} r={unit * (dragged ? 2.4 : 1.8)} fill={source.color} />
              <text className="map-label" x={x} y={-y + unit * 5} fontSize={unit * 3} textAnchor="middle">
                {source.name}
              </text>
              <circle
                className={`map-handle${dragged ? ' dragging' : ''}${movable ? '' : ' fixed'}`}
                cx={x}
                cy={-y}
                r={unit * 4.5}
                onPointerDown={movable ? (event) => beginDrag(event, index) : undefined}
                onPointerMove={movable ? continueDrag : undefined}
                onPointerUp={() => setDrag(null)}
                onPointerCancel={() => setDrag(null)}
              />
            </g>
          )
        })}
      </svg>
      <p className="footnote">
        From above, front of the room up. Squares are speakers, the ring is the listener. Drag a
        sound to move it.
      </p>
    </section>
  )
}
