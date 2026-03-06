import { LitElement, html, css } from 'lit';
import { customElement, property } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

@customElement('gain-slider')
export class GainSlider extends LitElement {
  static styles = css`
    :host { display: block; }
    .row { display: flex; justify-content: space-between; align-items: center; }
    label { color: #999; }
    .val { color: #4fc3f7; }
    input[type=range] { width: 120px; margin-left: 8px; accent-color: #4fc3f7; }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) { this._ctrl.store = s; }

  private _onInput(e: Event) {
    const val = parseFloat((e.target as HTMLInputElement).value) / 100;
    this.store.setMasterGain(val);
  }

  render() {
    const gain = this.store?.masterGain ?? 0.7;
    return html`
      <div class="row">
        <label>Gain</label>
        <input type="range" min="0" max="100" .value=${String(Math.round(gain * 100))}
          @input=${this._onInput}>
        <span class="val">${gain.toFixed(2)}</span>
      </div>
    `;
  }
}
