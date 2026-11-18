import {
  CHANNELS_NEEDED,
  type ChannelModeName,
  type RenderModeName,
  type SceneState,
} from '../protocol'

const RENDER_MODE_LABELS: Record<RenderModeName, { name: string; hint: string }> = {
  world_locked: { name: 'World-locked', hint: 'Speakers as microphones in the world' },
  vbap: { name: 'VBAP', hint: 'Panned for one listening position' },
  hrtf: { name: 'HRTF', hint: 'Binaural, for headphones' },
  dbap: { name: 'DBAP', hint: 'Panned by distance to each speaker' },
  ambisonics: { name: 'Ambisonics', hint: 'First-order sound field' },
}

const CHANNEL_MODE_LABELS: Record<ChannelModeName, string> = {
  stereo: 'Stereo',
  quad: 'Quad',
  '5.1': '5.1',
}

interface ModeSwitchProps {
  scene: SceneState
  renderMode: RenderModeName
  channelMode: ChannelModeName
  onRenderMode: (mode: RenderModeName, channelMode: ChannelModeName | null) => void
  onChannelMode: (mode: ChannelModeName) => void
}

/** Channel modes this render mode supports that also fit the output device. */
function usableChannelModes(scene: SceneState, mode: RenderModeName): ChannelModeName[] {
  const entry = scene.render_modes.find((candidate) => candidate.mode === mode)
  return (entry?.channel_modes ?? []).filter(
    (channelMode) => CHANNELS_NEEDED[channelMode] <= scene.total_channels,
  )
}

export function ModeSwitch({
  scene,
  renderMode,
  channelMode,
  onRenderMode,
  onChannelMode,
}: ModeSwitchProps) {
  const channelModes = usableChannelModes(scene, renderMode)

  return (
    <section className="card">
      <h2>Render mode</h2>
      <div className="mode-grid" role="radiogroup" aria-label="Render mode">
        {scene.render_modes.map(({ mode }) => {
          const usable = usableChannelModes(scene, mode)
          const active = mode === renderMode
          return (
            <button
              key={mode}
              type="button"
              role="radio"
              aria-checked={active}
              className={active ? 'mode active' : 'mode'}
              disabled={usable.length === 0}
              title={usable.length === 0 ? 'Needs more speakers than this scene has' : undefined}
              onClick={() => {
                // Keep the speaker set when the new mode supports it; otherwise
                // move to the largest one it does support.
                const nextChannelMode = usable.includes(channelMode)
                  ? null
                  : usable[usable.length - 1]
                onRenderMode(mode, nextChannelMode)
              }}
            >
              <span className="mode-name">{RENDER_MODE_LABELS[mode].name}</span>
              <span className="mode-hint">{RENDER_MODE_LABELS[mode].hint}</span>
            </button>
          )
        })}
      </div>

      <h2>Speakers in use</h2>
      <div className="segmented" role="radiogroup" aria-label="Speakers in use">
        {(Object.keys(CHANNEL_MODE_LABELS) as ChannelModeName[]).map((mode) => (
          <button
            key={mode}
            type="button"
            role="radio"
            aria-checked={mode === channelMode}
            className={mode === channelMode ? 'segment active' : 'segment'}
            disabled={!channelModes.includes(mode)}
            onClick={() => onChannelMode(mode)}
          >
            {CHANNEL_MODE_LABELS[mode]}
          </button>
        ))}
      </div>
    </section>
  )
}
