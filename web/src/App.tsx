import { useState } from 'react'
import { GainControl } from './components/GainControl'
import { LevelMeters } from './components/LevelMeters'
import { ModeSwitch } from './components/ModeSwitch'
import { SceneMap } from './components/SceneMap'
import { SourceList } from './components/SourceList'
import { useEngine, type ConnectionStatus } from './useEngine'

const STATUS_TEXT: Record<ConnectionStatus, string> = {
  connecting: 'Connecting',
  connected: 'Connected',
  disconnected: 'Engine not reachable',
}

/** How long a slider value outranks the engine's report, in milliseconds. */
const GAIN_SETTLE_MS = 400

function telemetryAge(since: number): number {
  return performance.now() - since
}

export default function App() {
  const { status, address, scene, telemetry, send } = useEngine()
  // The slider shows the engine's own gain. While a change is on its way the
  // chosen value wins, so the thumb does not jump back under the finger.
  const [chosenGain, setChosenGain] = useState<{ gain: number; at: number } | null>(null)
  const engineGain = telemetry?.master_gain ?? scene?.master_gain ?? 1
  const masterGain =
    chosenGain && telemetryAge(chosenGain.at) < GAIN_SETTLE_MS ? chosenGain.gain : engineGain

  return (
    <div className="app">
      <header className="header">
        <h1>Atrium</h1>
        <div className={`status ${status}`} role="status">
          <span className="status-dot" />
          <span>{STATUS_TEXT[status]}</span>
          <span className="status-address">{address.replace('ws://', '')}</span>
        </div>
      </header>

      {!scene ? (
        <main className="empty">
          <p>
            {status === 'disconnected'
              ? 'No engine is answering at this address.'
              : 'Waiting for the engine to send its scene.'}
          </p>
          <p className="footnote">
            Start it from the repository root: <code>cargo run --release</code>
          </p>
        </main>
      ) : (
        <main className="layout">
          <div className="column">
            <SceneMap scene={scene} live={telemetry?.sources ?? []} />
            <SourceList
              sources={scene.sources}
              live={telemetry?.sources ?? []}
              onMute={(index, muted) => send({ type: 'set_source_muted', index, muted })}
            />
          </div>
          <div className="column">
            <ModeSwitch
              scene={scene}
              // The telemetry stream carries the mode the engine is really in.
              renderMode={telemetry?.render_mode ?? scene.render_mode}
              channelMode={telemetry?.channel_mode ?? scene.channel_mode}
              onRenderMode={(mode, channelMode) => {
                if (channelMode) send({ type: 'set_channel_mode', mode: channelMode })
                send({ type: 'set_render_mode', mode })
              }}
              onChannelMode={(mode) => send({ type: 'set_channel_mode', mode })}
            />
            <LevelMeters scene={scene} peaksDb={telemetry?.peaks_db ?? []} />
            <GainControl
              gain={masterGain}
              onGain={(gain) => {
                setChosenGain({ gain, at: performance.now() })
                send({ type: 'set_gain', gain })
              }}
            />
            <section className="card">
              <button type="button" className="secondary" onClick={() => send({ type: 'reset_scene' })}>
                Reset scene
              </button>
              <p className="footnote">
                Puts positions, gains, mode and speakers back to what the scene file says.
              </p>
            </section>
          </div>
        </main>
      )}
    </div>
  )
}
