use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tungstenite::protocol::Message;

use crate::engine::commands::Command;

use super::messages::ClientMessage;
use super::ui::INDEX_HTML;

/// Shared handle to the command producer.
/// Arc<Mutex<_>> because each WebSocket handler thread needs access.
pub type SharedProducer = Arc<Mutex<rtrb::Producer<Command>>>;

/// Shared telemetry broadcast: the broadcaster thread writes the latest JSON,
/// WebSocket handler threads read it on idle (read timeout).
pub struct TelemetryBroadcast {
    json: Mutex<String>,
    generation: AtomicU64,
}

impl TelemetryBroadcast {
    pub fn new() -> Self {
        Self {
            json: Mutex::new(String::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Called by the broadcaster thread with new telemetry JSON.
    pub fn update(&self, new_json: String) {
        if let Ok(mut json) = self.json.lock() {
            *json = new_json;
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Called by WS handler threads. Returns Some(json) if there's a newer frame
    /// than the caller's last seen generation.
    pub fn latest(&self, last_gen: &mut u64) -> Option<String> {
        let gen = self.generation.load(Ordering::Acquire);
        if gen > *last_gen {
            *last_gen = gen;
            self.json.lock().ok().map(|j| j.clone())
        } else {
            None
        }
    }
}

/// Start the WebSocket/HTTP server. Blocks forever (runs on the main thread).
///
/// `initial_state` is a JSON string sent to each WebSocket client on connect
/// (e.g. speaker layout). Pass an empty string to skip.
pub fn run_server(
    addr: &str,
    producer: SharedProducer,
    initial_state: String,
    broadcast: Arc<TelemetryBroadcast>,
) -> std::io::Result<()> {
    let initial_state: Arc<String> = Arc::new(initial_state);
    let listener = TcpListener::bind(addr)?;
    println!("Control UI: http://{addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let producer = producer.clone();
                let initial_state = initial_state.clone();
                let broadcast = broadcast.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, producer, &initial_state, broadcast) {
                        eprintln!("Connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("Accept error: {e}"),
        }
    }
    Ok(())
}

fn handle_connection(
    stream: TcpStream,
    producer: SharedProducer,
    initial_state: &str,
    broadcast: Arc<TelemetryBroadcast>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Peek at the raw bytes to determine if this is a WebSocket upgrade
    // or a plain HTTP request for the UI page.
    let mut buf = [0u8; 4096];
    let n = stream.peek(&mut buf)?;
    let request = std::str::from_utf8(&buf[..n]).unwrap_or("");

    if request.contains("Upgrade: websocket") || request.contains("upgrade: websocket") {
        handle_websocket(stream, producer, initial_state, broadcast)
    } else {
        handle_http(stream)
    }
}

fn handle_http(mut stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        INDEX_HTML.len(),
        INDEX_HTML,
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn handle_websocket(
    stream: TcpStream,
    producer: SharedProducer,
    initial_state: &str,
    broadcast: Arc<TelemetryBroadcast>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Set read timeout so we can interleave telemetry sends between client reads
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut ws = tungstenite::accept(stream)?;
    let mut last_gen = 0u64;

    // Send initial state (speaker layout etc.) to the newly connected client
    if !initial_state.is_empty() {
        let _ = ws.send(Message::Text(initial_state.into()));
    }

    loop {
        match ws.read() {
            Ok(msg) => match msg {
                Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(client_msg) => {
                        let resend = client_msg.needs_scene_resend();
                        let cmd = client_msg.into_command();
                        if let Ok(mut prod) = producer.lock() {
                            if prod.push(cmd).is_err() {
                                eprintln!("command queue full, dropping command");
                            }
                        }
                        if resend && !initial_state.is_empty() {
                            let _ = ws.send(Message::Text(initial_state.into()));
                        }
                    }
                    Err(e) => {
                        eprintln!("Invalid message: {e}");
                    }
                },
                Message::Close(_) => break,
                Message::Ping(data) => {
                    let _ = ws.send(Message::Pong(data));
                }
                _ => {}
            },
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Read timeout — send telemetry if a new frame is available
                if let Some(json) = broadcast.latest(&mut last_gen) {
                    if ws.send(Message::Text(json.into())).is_err() {
                        break;
                    }
                }
            }
            Err(tungstenite::Error::ConnectionClosed) => break,
            Err(tungstenite::Error::Protocol(_)) => break,
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::ConnectionReset =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_producer_sends_command() {
        let (producer, mut consumer) = rtrb::RingBuffer::<Command>::new(16);
        let shared = Arc::new(Mutex::new(producer));

        // Simulate what the WebSocket handler does
        let cmd = Command::SetMasterGain { gain: 0.42 };
        {
            let mut prod = shared.lock().unwrap();
            prod.push(cmd).unwrap();
        }

        let received = consumer.pop().unwrap();
        match received {
            Command::SetMasterGain { gain } => {
                assert!((gain - 0.42).abs() < 1e-6);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn telemetry_broadcast_tracks_generation() {
        let bc = TelemetryBroadcast::new();
        let mut last_gen = 0;

        // No update yet — should return None
        assert!(bc.latest(&mut last_gen).is_none());

        bc.update(r#"{"type":"telemetry"}"#.to_string());
        let result = bc.latest(&mut last_gen);
        assert!(result.is_some());
        assert_eq!(last_gen, 1);

        // Same generation — should return None
        assert!(bc.latest(&mut last_gen).is_none());

        // New update
        bc.update(r#"{"type":"telemetry","v":2}"#.to_string());
        let result = bc.latest(&mut last_gen);
        assert!(result.is_some());
        assert_eq!(last_gen, 2);
    }
}
