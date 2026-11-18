import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Port 6006, not 6000: browsers refuse to open port 6000 (it is on their
// list of unsafe ports, reserved for X11), so a page served there never loads.
const PORT = Number(process.env.PORT) || 6006

// `host: true` serves on the local network too, so a phone on the same
// Wi-Fi can open the page. The page then talks to the engine's WebSocket on
// the same host name, port 3333.
export default defineConfig({
  plugins: [react()],
  server: { host: true, port: PORT, strictPort: true },
  preview: { host: true, port: PORT, strictPort: true },
})
