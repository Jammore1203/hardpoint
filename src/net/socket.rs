//! Non-blocking UDP socket wrapper.
//!
//! Deliberately thin. The one thing it adds beyond `UdpSocket` is that every
//! failure mode a game will actually hit - a full send buffer, an ICMP
//! rejection from a peer that has gone away, a truncated datagram - is turned
//! into something the caller can ignore rather than an error that stops the
//! loop.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use super::protocol::MAX_PACKET;

pub struct Socket {
    sock: UdpSocket,
    pub local: SocketAddr,
    buf: [u8; MAX_PACKET],
}

impl Socket {
    /// Binds to a port, or to the first free port in a range.
    pub fn bind(port: u16) -> std::io::Result<Socket> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let sock = UdpSocket::bind(addr)?;
        Socket::configure(sock)
    }

    pub fn bind_any() -> std::io::Result<Socket> { Socket::bind(0) }

    /// Tries each port in turn, which is what lets two servers run on one
    /// machine without configuration.
    pub fn bind_range(range: std::ops::Range<u16>) -> std::io::Result<Socket> {
        let mut last = None;
        for port in range {
            match Socket::bind(port) {
                Ok(s) => return Ok(s),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| std::io::Error::new(ErrorKind::AddrInUse, "no free port")))
    }

    fn configure(sock: UdpSocket) -> std::io::Result<Socket> {
        sock.set_nonblocking(true)?;
        // Broadcast is needed for LAN discovery; failing to enable it is not
        // fatal, it just means discovery will not work on this platform.
        let _ = sock.set_broadcast(true);
        let local = sock.local_addr()?;
        Ok(Socket { sock, local, buf: [0u8; MAX_PACKET] })
    }

    pub fn port(&self) -> u16 { self.local.port() }

    /// Receives one datagram, or `None` if nothing is waiting.
    pub fn recv(&mut self) -> Option<(&[u8], SocketAddr)> {
        loop {
            match self.sock.recv_from(&mut self.buf) {
                Ok((n, from)) => {
                    if n == 0 { continue; }
                    return Some((&self.buf[..n], from));
                }
                Err(e) => match e.kind() {
                    ErrorKind::WouldBlock => return None,
                    // A previous send hit an unreachable host. Windows
                    // surfaces this on the *next* receive; ignoring it and
                    // trying again is the correct response.
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused => continue,
                    _ => return None,
                },
            }
        }
    }

    pub fn send(&self, bytes: &[u8], to: SocketAddr) -> bool {
        if bytes.len() > MAX_PACKET { return false; }
        match self.sock.send_to(bytes, to) {
            Ok(n) => n == bytes.len(),
            Err(_) => false,
        }
    }

    /// Sends to the local broadcast address on a range of ports.
    pub fn broadcast(&self, bytes: &[u8], ports: std::ops::Range<u16>) {
        for port in ports {
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), port);
            let _ = self.sock.send_to(bytes, addr);
            // Also try loopback, so two instances on one machine find each
            // other even where broadcast to self is filtered.
            let lo = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
            let _ = self.sock.send_to(bytes, lo);
        }
    }
}

/// Parses "host", "host:port" or a bare port into a socket address.
pub fn parse_endpoint(s: &str, default_port: u16) -> Option<SocketAddr> {
    use std::net::ToSocketAddrs;
    let s = s.trim();
    if s.is_empty() { return None; }
    if let Ok(port) = s.parse::<u16>() {
        return Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    }
    if s.contains(':') {
        if let Ok(mut it) = s.to_socket_addrs() {
            return it.next();
        }
        return None;
    }
    let with_port = format!("{}:{}", s, default_port);
    with_port.to_socket_addrs().ok().and_then(|mut it| it.next())
}
