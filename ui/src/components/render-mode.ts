import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

const CHANNEL_LABELS: Record<string, string> = {
  stereo: 'Stereo',
  quad: 'Quad',
  '5.1': '5.1',
};

@customElement('render-mode')
export class RenderMode extends LitElement {
  static styles = css`
    :host { display: block; margin-top: 8px; border-top: 1px solid #333; padding-top: 6px; }
    h3 { font-size: 12px; color: #999; margin-bottom: 4px; }
    .mode-btns { display: flex; gap: 4px; margin-top: 4px; flex-wrap: wrap; }
    .mode-btn {
      background: none; border: 1px solid #555; color: #999; font-size: 10px;
      padding: 3px 8px; border-radius: 4px; cursor: pointer; font-family: inherit;
    }
    .mode-btn:hover { border-color: #888; color: #ccc; }
    .mode-btn.active { border-color: #4fc3f7; color: #4fc3f7; background: rgba(79,195,247,0.1); }
    .channel-btns { display: flex; gap: 3px; margin-top: 4px; flex-wrap: wrap; }
    .ch-btn {
      background: none; border: 1px solid #444; color: #777; font-size: 9px;
      padding: 2px 6px; border-radius: 3px; cursor: pointer; font-family: inherit;
    }
    .ch-btn:hover { border-color: #777; color: #aaa; }
    .ch-btn.active { border-color: #66bb6a; color: #66bb6a; background: rgba(102,187,106,0.1); }
    .exp-section { margin-top: 6px; }
    .exp-label { font-size: 10px; color: #777; margin-bottom: 2px; }
    .exp-btns { display: flex; gap: 3px; flex-wrap: wrap; }
    .exp-btn {
      background: none; border: 1px solid #444; color: #777; font-size: 9px;
      padding: 2px 6px; border-radius: 3px; cursor: pointer; font-family: inherit;
    }
    .exp-btn:hover { border-color: #777; color: #aaa; }
    .exp-btn.active { border-color: #ce93d8; color: #ce93d8; background: rgba(206,147,216,0.1); }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  private _onModeClick(mode: string) {
    this.store.setRenderMode(mode);
  }

  private _onChannelClick(mode: string) {
    this.store.setChannelMode(mode);
  }

  private _onExperimentClick(name: string, value: string) {
    this.store.setExperiment(name, value);
  }

  render() {
    const modes = this.store?.renderModes ?? [];
    const current = modes.find(m => m.mode === this.store?.renderMode);
    const channelModes = current?.channel_modes ?? [];
    const experiments = this.store?.experiments ?? [];
    return html`
      <h3>Render Mode</h3>
      <div class="mode-btns">
        ${modes.map(m => html`
          <button class="mode-btn ${this.store?.renderMode === m.mode ? 'active' : ''}"
            @click=${() => this._onModeClick(m.mode)}>${m.mode}</button>
        `)}
      </div>
      ${channelModes.length > 1 ? html`
        <div class="channel-btns">
          ${channelModes.map(ch => html`
            <button class="ch-btn ${this.store?.channelMode === ch ? 'active' : ''}"
              @click=${() => this._onChannelClick(ch)}>${CHANNEL_LABELS[ch] ?? ch}</button>
          `)}
        </div>
      ` : ''}
      ${experiments.map(exp => html`
        <div class="exp-section">
          <div class="exp-label">${exp.name}</div>
          <div class="exp-btns">
            ${exp.values.map(v => html`
              <button class="exp-btn ${this.store?.experimentValues[exp.name] === v ? 'active' : ''}"
                @click=${() => this._onExperimentClick(exp.name, v)}>${v}</button>
            `)}
          </div>
        </div>
      `)}
    `;
  }
}
