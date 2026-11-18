import type { SceneSource, SourceTelemetry } from '../protocol'

interface SourceListProps {
  sources: SceneSource[]
  live: SourceTelemetry[]
  onMute: (index: number, muted: boolean) => void
}

/** Every sound in the scene, with how far it is and a switch to mute it. */
export function SourceList({ sources, live, onMute }: SourceListProps) {
  return (
    <section className="card">
      <h2>Sounds</h2>
      <ul className="sources">
        {sources.map((source, index) => {
          const state = live[index]
          const muted = state?.muted ?? false
          return (
            <li key={`${source.name}-${index}`} className={muted ? 'source muted' : 'source'}>
              <span className="swatch" style={{ background: source.color }} />
              <span className="source-name">
                {source.name}
                <span className="source-kind">
                  {source.emitter_kind === 'field' ? 'all around' : state ? `${state.distance.toFixed(1)} m` : ''}
                </span>
              </span>
              <span className="source-level">{source.spl.toFixed(0)} dB SPL</span>
              <button
                type="button"
                className={muted ? 'toggle' : 'toggle on'}
                aria-pressed={!muted}
                onClick={() => onMute(index, !muted)}
              >
                {muted ? 'Muted' : 'On'}
              </button>
            </li>
          )
        })}
      </ul>
    </section>
  )
}
