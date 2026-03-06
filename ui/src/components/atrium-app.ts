import { LitElement, html, css } from 'lit';
import { customElement } from 'lit/decorators.js';
import type { AtriumStore } from '../store.js';

import './hud-panel.js';
import './help-overlay.js';

@customElement('atrium-app')
export class AtriumApp extends LitElement {
  static styles = css`
    :host { display: block; }
  `;

  store?: AtriumStore;

  setStore(store: AtriumStore) {
    this.store = store;
    this.requestUpdate();
  }

  render() {
    if (!this.store) return html``;
    return html`
      <hud-panel .store=${this.store}></hud-panel>
      <help-overlay></help-overlay>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'atrium-app': AtriumApp;
  }
}
