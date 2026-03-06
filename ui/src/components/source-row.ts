import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';
import { getSourceGainText } from '../three/sources.js';

@customElement('source-row')
export class SourceRow extends LitElement {
  static styles = css`
    :host { display: block; }
    .row { display: flex; justify-content: space-between; align-items: center; margin-top: 4px; }
    .source-name { font-size: 11px; }
    .mute-btn {
      background: none; border: 1px solid #555; color: #999; font-size: 10px;
      padding: 1px 6px; border-radius: 4px; cursor: pointer; font-family: inherit;
    }
    .mute-btn:hover { border-color: #888; color: #ccc; }
    .mute-btn.muted { border-color: #f44336; color: #f44336; }
    .pause-btn {
      background: none; border: 1px solid #555; color: #4fc3f7; font-size: 10px;
      padding: 1px 6px; border-radius: 4px; cursor: pointer; font-family: inherit;
    }
    .pause-btn:hover { border-color: #4fc3f7; }
    .pause-btn.paused { border-color: #ff9800; color: #ff9800; }
    .gain-readout {
      font-size: 10px; color: #666; margin-top: 1px;
      font-variant-numeric: tabular-nums;
    }
    .orbit-controls { margin: 2px 0 4px; }
    .orbit-controls .ctrl-row { display: flex; align-items: center; gap: 4px; margin-top: 2px; }
    .orbit-controls label { font-size: 10px; color: #777; min-width: 40px; }
    .orbit-controls input[type=range] { width: 70px; accent-color: #4fc3f7; }
    .orbit-controls .val { font-size: 10px; color: #aaa; min-width: 30px; }
    .buttons { display: flex; gap: 4px; }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  @property({ type: Number }) index = 0;

  private _toggleMute() {
    this.store.setSourceMuted(this.index, !this.store.sourceMuted[this.index]);
  }

  private _togglePause() {
    this.store.toggleSourcePause(this.index);
  }

  private _onRadius(e: Event) {
    const val = parseFloat((e.target as HTMLInputElement).value);
    this.store.setSourceOrbitRadius(this.index, val);
    this.requestUpdate();
  }

  private _onAngle(e: Event) {
    const deg = parseFloat((e.target as HTMLInputElement).value);
    const rad = deg * Math.PI / 180;
    this.store.setSourceOrbitAngle(this.index, rad);
    this.requestUpdate();
  }

  render() {
    const s = this.store?.sources[this.index];
    if (!s) return html``;
    const muted = this.store.sourceMuted[this.index];
    const paused = this.store.sourcePaused[this.index];
    const hasOrbit = s.r > 0 || s.speed !== 0;
    const hexColor = '#' + s.color.toString(16).padStart(6, '0');

    return html`
      <div class="row">
        <span class="source-name" style="color:${hexColor}">${s.name}</span>
        <div class="buttons">
          <button class="mute-btn ${muted ? 'muted' : ''}" @click=${this._toggleMute}>
            ${muted ? 'muted' : 'mute'}
          </button>
          ${(s.r > 0 && this.store.sourceOrigSpeed[this.index] !== 0) ? html`
            <button class="pause-btn ${paused ? 'paused' : ''}" @click=${this._togglePause}>
              ${paused ? '\u25B6' : '\u23F8'}
            </button>
          ` : ''}
        </div>
      </div>
      ${hasOrbit ? html`
        <div class="orbit-controls">
          <div class="ctrl-row">
            <label>Radius</label>
            <input type="range" min="0.2" max="3" step="0.1" .value=${String(s.r)}
              @input=${this._onRadius}>
            <span class="val">${s.r.toFixed(1)}m</span>
          </div>
          <div class="ctrl-row">
            <label>Angle</label>
            <input type="range" min="0" max="360" step="1" .value=${"0"}
              @input=${this._onAngle}>
            <span class="val">0&deg;</span>
          </div>
        </div>
      ` : ''}
      <div class="gain-readout">${getSourceGainText(this.store, this.index)}</div>
    `;
  }
}
