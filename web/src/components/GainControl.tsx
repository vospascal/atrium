import { MAX_MASTER_GAIN } from '../protocol'

const MINIMUM_DB = -40
const MAXIMUM_DB = 20 * Math.log10(MAX_MASTER_GAIN)

interface GainControlProps {
  gain: number
  onGain: (gain: number) => void
}

function gainToDb(gain: number): number {
  return gain <= 0 ? MINIMUM_DB : Math.max(MINIMUM_DB, 20 * Math.log10(gain))
}

/** Master gain as a decibel slider; the bottom of the travel is silence. */
export function GainControl({ gain, onGain }: GainControlProps) {
  const decibels = gainToDb(gain)
  const silent = decibels <= MINIMUM_DB
  return (
    <section className="card">
      <h2>
        Master gain
        <span className="value">{silent ? 'off' : `${decibels >= 0 ? '+' : ''}${decibels.toFixed(1)} dB`}</span>
      </h2>
      <input
        type="range"
        aria-label="Master gain in decibels"
        min={MINIMUM_DB}
        max={MAXIMUM_DB}
        step={0.5}
        value={decibels}
        onChange={(event) => {
          const next = Number(event.target.value)
          onGain(next <= MINIMUM_DB ? 0 : 10 ** (next / 20))
        }}
      />
      <div className="range-ends">
        <span>off</span>
        <button type="button" className="link" onClick={() => onGain(1)}>
          0 dB
        </button>
        <span>+{MAXIMUM_DB.toFixed(0)} dB</span>
      </div>
    </section>
  )
}
