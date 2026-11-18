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
  /** Channels the output device opened with; fewer than the scene's speakers on a laptop. */
  deviceChannels: number
  renderMode: RenderModeName
  channelMode: ChannelModeName
  onRenderMode: (mode: RenderModeName, channelMode: ChannelModeName | null) => void
  onChannelMode: (mode: ChannelModeName) => void
}

/** Speaker sets this render mode supports that the scene and the output device both have. */
function usableChannelModes(
  scene: SceneState,
  deviceChannels: number,
  mode: RenderModeName,
): ChannelModeName[] {
  const entry = scene.render_modes.find((candidate) => candidate.mode === mode)
  const channelsAvailable = Math.min(scene.total_channels, deviceChannels)
  return (entry?.channel_modes ?? []).filter(
    (channelMode) => CHANNELS_NEEDED[channelMode] <= channelsAvailable,
  )
}

export function ModeSwitch({
  scene,
  deviceChannels,
  renderMode,
  channelMode,
  onRenderMode,
  onChannelMode,
}: ModeSwitchProps) {
  const channelModes = usableChannelModes(scene, deviceChannels, renderMode)
  const deviceIsSmaller = deviceChannels < scene.total_channels

  return (
    <section className="card">
      <h2>Render mode</h2>
      <div className="mode-grid" role="radiogroup" aria-label="Render mode">
        {scene.render_modes.map(({ mode }) => {
          const usable = usableChannelModes(scene, deviceChannels, mode)
          const active = mode === renderMode
          return (
            <button
              key={mode}
              type="button"
              role="radio"
              aria-checked={active}
              className={active ? 'mode active' : 'mode'}
              onClick={() => {
                // Keep the speaker set when the new mode supports it; otherwise
                // move to the largest one it does support on this device. If
                // there is none, the engine plays the mode on what is there.
                const nextChannelMode =
                  usable.includes(channelMode) || usable.length === 0
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
      {deviceIsSmaller && (
        <p className="footnote">
          This output device has {deviceChannels} channels, so only the first {deviceChannels} of
          the scene's {scene.total_channels} speaker channels can play here. The larger sets need
          a device with more outputs.
        </p>
      )}
    </section>
  )
}
