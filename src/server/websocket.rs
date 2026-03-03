use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use tungstenite::protocol::Message;

use crate::engine::commands::Command;

use super::messages::ClientMessage;
use super::ui::INDEX_HTML;

/// Shared handle to the command producer.
/// Arc<Mutex<_>> because each WebSocket handler thread needs access.
pub type SharedProducer = Arc<Mutex<rtrb::Producer<Command>>>;

/// Start the WebSocket/HTTP server. Blocks forever (runs on the main thread).
pub fn run_server(addr: &str, producer: SharedProducer) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    println!("Control UI: http://{addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let producer = producer.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, producer) {
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
) -> Result<(), Box<dyn std::error::Error>> {
    // Peek at the raw bytes to determine if this is a WebSocket upgrade
    // or a plain HTTP request for the UI page.
    let mut buf = [0u8; 4096];
    let n = stream.peek(&mut buf)?;
    let request = std::str::from_utf8(&buf[..n]).unwrap_or("");

    if request.contains("Upgrade: websocket") || request.contains("upgrade: websocket") {
        handle_websocket(stream, producer)
    } else {
        handle_http(stream)
    }
}

fn handle_http(mut stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Read and discard the full HTTP request
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);

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
) -> Result<(), Box<dyn std::error::Error>> {
    let mut ws = tungstenite::accept(stream)?;

    loop {
        let msg = match ws.read() {
            Ok(msg) => msg,
            Err(tungstenite::Error::ConnectionClosed) => break,
            Err(tungstenite::Error::Protocol(_)) => break,
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::ConnectionReset =>
            {
                break
            }
            Err(e) => return Err(e.into()),
        };

        match msg {
            Message::Text(text) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(client_msg) => {
                        let cmd = client_msg.into_command();
                        if let Ok(mut prod) = producer.lock() {
                            let _ = prod.push(cmd); // silent drop if ring buffer full
                        }
                    }
                    Err(e) => {
                        eprintln!("Invalid message: {e}");
                    }
                }
            }
            Message::Close(_) => break,
            Message::Ping(data) => {
                let _ = ws.send(Message::Pong(data));
            }
            _ => {} // ignore binary, pong
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
}
