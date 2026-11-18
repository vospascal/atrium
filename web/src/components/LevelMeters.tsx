import type { SceneState } from '../protocol'

const METER_FLOOR_DB = -60

interface LevelMetersProps {
  scene: SceneState
  peaksDb: number[]
}

function channelLabel(scene: SceneState, channel: number): string {
  if (scene.lfe_channel === channel) return 'LFE'
  return scene.speakers.find((speaker) => speaker.channel === channel)?.label ?? `${channel + 1}`
}

/** One bar per output channel: peak level in dBFS over the last buffer. */
export function LevelMeters({ scene, peaksDb }: LevelMetersProps) {
  return (
    <section className="card">
      <h2>Output</h2>
      <div className="meters">
        {peaksDb.map((peakDb, channel) => {
          const fraction = Math.min(1, Math.max(0, (peakDb - METER_FLOOR_DB) / -METER_FLOOR_DB))
          const tone = peakDb > -1 ? 'clip' : peakDb > -12 ? 'hot' : 'ok'
          return (
            <div className="meter" key={channel}>
              <div className="meter-track">
                <div className={`meter-fill ${tone}`} style={{ height: `${fraction * 100}%` }} />
              </div>
              <span className="meter-value">
                {peakDb <= METER_FLOOR_DB ? '—' : peakDb.toFixed(0)}
              </span>
              <span className="meter-label">{channelLabel(scene, channel)}</span>
            </div>
          )
        })}
      </div>
      <p className="footnote">Peak per channel, dBFS. 0 is full scale.</p>
    </section>
  )
}
