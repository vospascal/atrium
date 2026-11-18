import type { SceneState, SourceTelemetry } from '../protocol'

interface SceneMapProps {
  scene: SceneState
  live: SourceTelemetry[]
}

interface Bounds {
  minX: number
  maxX: number
  minY: number
  maxY: number
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
    const reach = Math.abs(source.orbit_radius)
    xs.push(source.position[0] - reach, source.position[0] + reach)
    ys.push(source.position[1] - reach, source.position[1] + reach)
    const state = live[index]
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
 * y is negated on the way into SVG, whose y axis points down.
 */
export function SceneMap({ scene, live }: SceneMapProps) {
  const bounds = viewBounds(scene, live)
  const size = bounds.maxX - bounds.minX
  const unit = size / 100 // one hundredth of the view: keeps marks the same size at any zoom
  const forward = { x: Math.cos(scene.listener.yaw), y: Math.sin(scene.listener.yaw) }

  return (
    <section className="card map-card">
      <h2>
        Scene
        <span className="value">{size.toFixed(0)} m across</span>
      </h2>
      <svg
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
          const x = state?.x ?? source.position[0]
          const y = state?.y ?? source.position[1]
          return (
            <g key={`${source.name}-${index}`} opacity={state?.muted ? 0.3 : 1}>
              <circle cx={x} cy={-y} r={unit * 1.8} fill={source.color} />
              <text className="map-label" x={x} y={-y + unit * 5} fontSize={unit * 3} textAnchor="middle">
                {source.name}
              </text>
            </g>
          )
        })}
      </svg>
      <p className="footnote">
        From above, front of the room up. Squares are speakers, the ring is the listener.
      </p>
    </section>
  )
}
