//! A small WebSocket client for tests: the live-reload channel, and Chrome's DevTools
//! protocol. Text messages only; enough of RFC 6455 for those two peers.

#![allow(dead_code)]

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    /// Open `ws://{addr}{path}`, sending `origin` as the page's origin when given.
    pub fn connect(addr: SocketAddr, path: &str, origin: Option<&str>) -> io::Result<Client> {
        let stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        let mut writer = stream.try_clone()?;
        let origin = origin
            .map(|o| format!("Origin: {o}\r\n"))
            .unwrap_or_default();
        write!(
            writer,
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{origin}\r\n"
        )?;
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status)?;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        if !status.starts_with("HTTP/1.1 101") {
            return Err(io::Error::other(format!("upgrade refused: {status}")));
        }
        Ok(Client { reader, writer })
    }

    pub fn set_timeout(&self, t: Duration) {
        let _ = self.writer.set_read_timeout(Some(t));
    }

    /// Send a text message, masked as a client must.
    pub fn send(&mut self, text: &str) -> io::Result<()> {
        let payload = text.as_bytes();
        let mut f = vec![0x81u8];
        let n = payload.len();
        if n < 126 {
            f.push(0x80 | n as u8);
        } else if n <= u16::MAX as usize {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.writer.write_all(&f)
    }

    /// The next text message. Pings are answered; a close or a dropped connection is an error.
    pub fn recv(&mut self) -> io::Result<String> {
        let mut message = Vec::new();
        loop {
            let mut head = [0u8; 2];
            self.reader.read_exact(&mut head)?;
            let len = match head[1] & 0x7f {
                126 => {
                    let mut b = [0u8; 2];
                    self.reader.read_exact(&mut b)?;
                    u16::from_be_bytes(b) as usize
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.reader.read_exact(&mut b)?;
                    u64::from_be_bytes(b) as usize
                }
                n => n as usize,
            };
            let mut payload = vec![0u8; len];
            self.reader.read_exact(&mut payload)?;
            match head[0] & 0x0f {
                0x8 => return Err(io::ErrorKind::ConnectionAborted.into()),
                0x9 => {
                    let mut pong = vec![0x8Au8, 0x80 | payload.len() as u8, 0, 0, 0, 0];
                    pong.extend_from_slice(&payload);
                    self.writer.write_all(&pong)?;
                    continue;
                }
                0xA => continue,
                _ => {}
            }
            message.extend_from_slice(&payload);
            if head[0] & 0x80 != 0 {
                return String::from_utf8(message).map_err(io::Error::other);
            }
        }
    }
}
