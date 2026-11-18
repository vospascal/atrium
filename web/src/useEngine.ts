import { useCallback, useEffect, useRef, useState } from 'react'
import type { ClientMessage, SceneState, ServerMessage, Telemetry } from './protocol'

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected'

const RECONNECT_DELAY_MS = 1500

/**
 * Where the engine listens. By default the same host that served this page,
 * port 3333; `?engine=host:port` in the address bar overrides it.
 */
export function engineAddress(): string {
  const override = new URLSearchParams(window.location.search).get('engine')
  if (override) {
    return override.includes('://') ? override : `ws://${override}`
  }
  return `ws://${window.location.hostname || 'localhost'}:3333`
}

export interface Engine {
  status: ConnectionStatus
  address: string
  scene: SceneState | null
  telemetry: Telemetry | null
  send: (message: ClientMessage) => void
}

/** One WebSocket to the engine, reconnecting for as long as the page is open. */
export function useEngine(): Engine {
  const [status, setStatus] = useState<ConnectionStatus>('connecting')
  const [scene, setScene] = useState<SceneState | null>(null)
  const [telemetry, setTelemetry] = useState<Telemetry | null>(null)
  const socketRef = useRef<WebSocket | null>(null)
  const address = engineAddress()

  useEffect(() => {
    let closedByUs = false
    let reconnectTimer: number | undefined

    const connect = () => {
      setStatus('connecting')
      const socket = new WebSocket(address)
      socketRef.current = socket

      socket.onopen = () => setStatus('connected')
      socket.onmessage = (event) => {
        let message: ServerMessage
        try {
          message = JSON.parse(event.data as string) as ServerMessage
        } catch {
          return
        }
        if (message.type === 'scene_state') {
          setScene(message)
        } else if (message.type === 'telemetry') {
          setTelemetry(message)
        }
      }
      socket.onclose = () => {
        if (closedByUs) return
        setStatus('disconnected')
        setTelemetry(null)
        reconnectTimer = window.setTimeout(connect, RECONNECT_DELAY_MS)
      }
      socket.onerror = () => socket.close()
    }

    connect()
    return () => {
      closedByUs = true
      window.clearTimeout(reconnectTimer)
      socketRef.current?.close()
    }
  }, [address])

  const send = useCallback((message: ClientMessage) => {
    const socket = socketRef.current
    if (socket && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(message))
    }
  }, [])

  return { status, address, scene, telemetry, send }
}
