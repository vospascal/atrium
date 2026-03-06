import { LitElement, html, css } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';
import { StoreController } from '../store-controller.js';

/** ISO 9613-1 absorption coefficient (client-side for display) */
function iso9613Alpha(freq: number, tempC: number, humPct: number): number {
  const T_REF = 293.15, T_TRIPLE = 273.16;
  const tK = tempC + 273.15;
  const tRel = tK / T_REF;
  const pSatRatio = Math.pow(10, -6.8346 * Math.pow(T_TRIPLE / tK, 1.261) + 4.6151);
  const h = humPct * pSatRatio;
  const frO = 24.0 + 4.04e4 * h * (0.02 + h) / (0.391 + h);
  const frN = Math.pow(tRel, -0.5) * (9.0 + 280.0 * h * Math.exp(-4.170 * (Math.pow(tRel, -1 / 3) - 1.0)));
  const f2 = freq * freq;
  const classical = 1.84e-11 * Math.sqrt(tRel);
  const vibO2 = Math.pow(tRel, -2.5) * 0.01275 * Math.exp(-2239.1 / tK) / (frO + f2 / frO);
  const vibN2 = Math.pow(tRel, -2.5) * 0.1068 * Math.exp(-3352.0 / tK) / (frN + f2 / frN);
  const alpha = 8.686 * f2 * (classical + vibO2 + vibN2);
  return isFinite(alpha) ? alpha : 0;
}

@customElement('atmosphere-panel')
export class AtmospherePanel extends LitElement {
  static styles = css`
    :host { display: block; margin-top: 8px; border-top: 1px solid #333; padding-top: 6px; }
    h3 { font-size: 12px; color: #999; margin-bottom: 4px; }
    .row { display: flex; justify-content: space-between; align-items: center; }
    label { color: #999; }
    .val { color: #4fc3f7; }
    input[type=range] { width: 80px; accent-color: #4fc3f7; }
    .readout {
      font-size: 10px; color: #666; margin-top: 4px;
      font-variant-numeric: tabular-nums;
    }
  `;

  private _ctrl = new StoreController(this);

  @property({ attribute: false })
  get store(): AtriumStore { return this._ctrl.store!; }
  set store(s: AtriumStore) {
    this._ctrl.store = s;
    if (s) {
      this.temp = s.atmosphere.temperature_c;
      this.humidity = s.atmosphere.humidity_pct;
    }
  }

  @state() private temp = 20;
  @state() private humidity = 50;

  private _onTemp(e: Event) {
    this.temp = parseFloat((e.target as HTMLInputElement).value);
    this.store.setAtmosphere(this.temp, this.humidity);
  }

  private _onHumidity(e: Event) {
    this.humidity = parseFloat((e.target as HTMLInputElement).value);
    this.store.setAtmosphere(this.temp, this.humidity);
  }

  render() {
    const a1k = iso9613Alpha(1000, this.temp, this.humidity).toFixed(3);
    const a4k = iso9613Alpha(4000, this.temp, this.humidity).toFixed(3);
    const a8k = iso9613Alpha(8000, this.temp, this.humidity).toFixed(3);

    return html`
      <h3>Atmosphere</h3>
      <div class="row">
        <label>Temp</label>
        <input type="range" min="-10" max="45" step="1" .value=${String(this.temp)}
          @input=${this._onTemp}>
        <span class="val">${this.temp}&deg;C</span>
      </div>
      <div class="row">
        <label>Humidity</label>
        <input type="range" min="0" max="100" step="1" .value=${String(this.humidity)}
          @input=${this._onHumidity}>
        <span class="val">${this.humidity}%</span>
      </div>
      <div class="readout">
        &alpha;: 1kHz ${a1k} &middot; 4kHz ${a4k} &middot; 8kHz ${a8k} dB/m
      </div>
    `;
  }
}
