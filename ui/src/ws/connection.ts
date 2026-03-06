import type { AtriumStore } from '../store.js';
import type { ServerMessage } from '../types.js';

export class WebSocketConnection {
  private ws: WebSocket | null = null;
  private store: AtriumStore;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(store: AtriumStore) {
    this.store = store;
    store.setSendFn((msg) => this.send(msg));
  }

  connect() {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    this.ws = new WebSocket(`${protocol}//${location.host}/ws`);

    this.ws.onopen = () => {
      this.store.setConnected(true);
      this.store.sendCurrentState();
    };

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data) as ServerMessage;
        if (msg.type === 'scene_state') this.store.handleSceneState(msg);
        else if (msg.type === 'speaker_layout') this.store.handleSpeakerLayout(msg);
        else if (msg.type === 'telemetry') this.store.handleTelemetry(msg.sources);
      } catch {
        // ignore non-JSON or unknown messages
      }
    };

    this.ws.onclose = () => {
      this.store.setConnected(false);
      this.reconnectTimer = setTimeout(() => this.connect(), 1000);
    };

    this.ws.onerror = () => this.ws?.close();
  }

  send(msg: object) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  disconnect() {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
  }
}
