import { LitElement, html, css } from 'lit';
import { customElement } from 'lit/decorators.js';

@customElement('help-overlay')
export class HelpOverlay extends LitElement {
  static styles = css`
    :host {
      position: absolute; bottom: 16px; left: 16px;
      background: rgba(0,0,0,0.7); padding: 10px 14px; border-radius: 8px;
      font-size: 11px; color: #666; line-height: 1.5;
      font-family: 'SF Mono', 'Fira Code', monospace;
      pointer-events: none;
    }
  `;

  render() {
    return html`
      Drag listener (white) or sources to move &middot;
      Space+Scroll to rotate yaw &middot;
      Scroll to zoom &middot;
      Right-drag to orbit &middot;
      Space+Right-drag to pan
    `;
  }
}
