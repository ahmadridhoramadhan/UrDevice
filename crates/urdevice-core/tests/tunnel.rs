//! Localhost sharing (#22, R13, design §11), end to end between two
//! in-process nodes over QUIC. `a` is the host that shares ports, `b` the
//! viewer. The target servers are plain TCP servers on the test's loopback.

#![allow(clippy::unwrap_used)]

mod harness;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use harness::{Pair, WAIT, wait_until};
use urdevice_core::services::tunnel::{LanMode, Tunnel, TunnelStatus, TunnelView};
use urdevice_core::{Capability, Mode};

fn svc(d: &harness::Dev) -> Arc<Tunnel> {
    d.node.service::<Tunnel>().unwrap()
}

/// A loopback TCP server that runs `f` for every connection and counts them.
fn server(f: fn(TcpStream)) -> (u16, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    thread::spawn(move || {
        for s in l.incoming().flatten() {
            h.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || f(s));
        }
    });
    (port, hits)
}

/// Echo until the client half-closes, then close.
fn echo(mut s: TcpStream) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if s.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = s.shutdown(Shutdown::Write);
}

fn tunnel(t: &Tunnel, id: u64) -> Option<TunnelView> {
    t.tunnels().into_iter().find(|v| v.id == id)
}

/// Open `port` on the host from the viewer and wait until it listens.
fn open(p: &Pair, port: u16) -> (u64, u16) {
    let b = svc(&p.b);
    let id = b.open(p.a.id(), port).unwrap();
    wait_until("tunnel open", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.status == TunnelStatus::Open)
    });
    (id, tunnel(&b, id).unwrap().local_port.unwrap())
}

fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(WAIT))).unwrap();
    s
}

fn shared_pair() -> (Pair, u16) {
    let p = Pair::new();
    p.a.set(Capability::Tunnel, Mode::Allow);
    let (port, _) = server(echo);
    svc(&p.a).share(port, "dev server").unwrap();
    let (a_id, b) = (p.a.id(), svc(&p.b));
    wait_until("list on the viewer", WAIT, || {
        b.remote_ports(a_id).iter().any(|r| r.port == port)
    });
    (p, port)
}

#[test]
fn localhost_is_deny_by_default_and_hides_the_list() {
    let p = Pair::new();
    assert_eq!(p.b.remote_mode(&p.a, Capability::Tunnel), Some(Mode::Deny));
    let (port, hits) = server(echo);
    svc(&p.a).share(port, "").unwrap();
    // The host shares it, but sends no list while Localhost is Deny.
    std::thread::sleep(Duration::from_millis(500));
    assert!(svc(&p.b).remote_ports(p.a.id()).is_empty());

    let b = svc(&p.b);
    let id = b.open(p.a.id(), port).unwrap();
    wait_until("denied", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.status == TunnelStatus::Failed("denied".into()))
    });
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    // Allow sends the list within the housekeeping tick.
    p.a.set(Capability::Tunnel, Mode::Allow);
    let a_id = p.a.id();
    wait_until("list after Allow", WAIT, || {
        b.remote_ports(a_id).iter().any(|r| r.port == port)
    });
    // And Deny clears it again.
    p.a.set(Capability::Tunnel, Mode::Deny);
    wait_until("list cleared", WAIT, || b.remote_ports(a_id).is_empty());
}

#[test]
fn bytes_flow_both_ways_with_half_close_and_stats() {
    let (p, port) = shared_pair();
    let label = &svc(&p.b).remote_ports(p.a.id())[0].label;
    assert_eq!(label, "dev server");
    let (id, local) = open(&p, port);

    // 1 MiB up, echoed back, with the write side closed first.
    let mut s = connect(local);
    let data: Vec<u8> = (0..1024 * 1024).map(|i| (i * 31 % 251) as u8).collect();
    let mut w = s.try_clone().unwrap();
    let d2 = data.clone();
    let writer = thread::spawn(move || {
        w.write_all(&d2).unwrap();
        w.shutdown(Shutdown::Write).unwrap();
    });
    let mut back = Vec::new();
    s.read_to_end(&mut back).unwrap();
    writer.join().unwrap();
    assert_eq!(back.len(), data.len());
    assert!(back == data);

    let b = svc(&p.b);
    wait_until("viewer stats", WAIT, || {
        tunnel(&b, id)
            .is_some_and(|v| v.bytes_up == data.len() as u64 && v.bytes_down == data.len() as u64)
    });
    // The host lists the incoming tunnel with the same traffic.
    let a = svc(&p.a);
    wait_until("host stats", WAIT, || {
        a.tunnels()
            .iter()
            .any(|v| v.incoming && v.remote_port == port && v.bytes_up == data.len() as u64)
    });
}

#[test]
fn many_connections_at_once() {
    let (p, port) = shared_pair();
    let (id, local) = open(&p, port);
    let threads: Vec<_> = (0..24)
        .map(|i| {
            thread::spawn(move || {
                let mut s = connect(local);
                let msg = format!("hello {i}\n");
                s.write_all(msg.as_bytes()).unwrap();
                let mut line = String::new();
                BufReader::new(s.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                assert_eq!(line, msg);
                s
            })
        })
        .collect();
    let open: Vec<TcpStream> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let b = svc(&p.b);
    wait_until("24 connections", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.connections == 24)
    });
    drop(open);
    wait_until("closed", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.connections == 0)
    });
}

#[test]
fn ports_that_are_not_shared_are_refused() {
    let (p, _shared) = shared_pair();
    // A server listens here, but the host never shared it.
    let (other, hits) = server(echo);
    let b = svc(&p.b);
    let id = b.open(p.a.id(), other).unwrap();
    wait_until("open", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.status == TunnelStatus::Open)
    });
    let local = tunnel(&b, id).unwrap().local_port.unwrap();
    let mut s = connect(local);
    let _ = s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    assert!(buf.is_empty(), "no bytes from a port that is not shared");
    wait_until("refused", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.status == TunnelStatus::Failed("refused_port".into()))
    });
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the host never connected");
}

#[test]
fn unsharing_closes_open_tunnels() {
    let (p, port) = shared_pair();
    let (id, local) = open(&p, port);
    let mut s = connect(local);
    s.write_all(b"ping\n").unwrap();
    let mut line = String::new();
    BufReader::new(s.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    svc(&p.a).unshare(port).unwrap();
    let b = svc(&p.b);
    wait_until("viewer tunnel gone", WAIT, || tunnel(&b, id).is_none());
    let a_id = p.a.id();
    wait_until("list empty", WAIT, || b.remote_ports(a_id).is_empty());
    // The open connection is closed too.
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    assert!(rest.is_empty());
}

#[test]
fn always_ask_prompts_once_per_tunnel_and_stop_works_from_the_host() {
    let (p, port) = shared_pair();
    p.a.set(Capability::Tunnel, Mode::Ask);
    let b = svc(&p.b);
    let id = b.open(p.a.id(), port).unwrap();
    let prompt = p.a.answer_prompt(true);
    assert_eq!(prompt.capability, "tunnel.v1");
    wait_until("open", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.status == TunnelStatus::Open)
    });
    let local = tunnel(&b, id).unwrap().local_port.unwrap();
    // Several connections, one approval.
    for _ in 0..3 {
        let mut s = connect(local);
        s.write_all(b"x\n").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        assert_eq!(line, "x\n");
    }
    // Stop on the host ends it on both devices and frees the listener.
    let a = svc(&p.a);
    let host_id = a.tunnels().iter().find(|v| v.incoming).unwrap().id;
    a.stop(host_id);
    wait_until("viewer closed", WAIT, || tunnel(&b, id).is_none());

    // A declined request fails with `declined`.
    let id = b.open(p.a.id(), port).unwrap();
    p.a.answer_prompt(false);
    wait_until("declined", WAIT, || {
        tunnel(&b, id).is_some_and(|v| v.status == TunnelStatus::Failed("declined".into()))
    });
}

#[test]
fn deny_closes_running_tunnels() {
    let (p, port) = shared_pair();
    let (id, _) = open(&p, port);
    p.a.set(Capability::Tunnel, Mode::Deny);
    let b = svc(&p.b);
    wait_until("closed by Deny", WAIT, || tunnel(&b, id).is_none());
}

#[test]
fn taken_local_port_uses_the_next_free_one() {
    let (p, port) = shared_pair();
    // The viewer's own copy of `port` is taken (the echo server runs on this
    // same machine), so the tunnel must pick another.
    let (_, local) = open(&p, port);
    assert_ne!(local, port);
    let mut s = connect(local);
    s.write_all(b"z\n").unwrap();
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    assert_eq!(line, "z\n");
}

/// A WebSocket-style upgrade: request, `101` reply, then server-pushed
/// frames and client frames on the same long-lived connection, like a dev
/// server's hot-reload channel.
#[test]
fn websocket_style_upgrade_and_server_push() {
    fn ws(mut s: TcpStream) {
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
        }
        s.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .unwrap();
        // Push three "reload" frames without being asked.
        for i in 0..3u8 {
            s.write_all(&[0x81, 7, b'r', b'e', b'l', b'o', b'a', b'd', b'0' + i])
                .unwrap();
            thread::sleep(Duration::from_millis(100));
        }
        // Then echo one client frame.
        let mut f = [0u8; 4];
        r.read_exact(&mut f).unwrap();
        s.write_all(&f).unwrap();
    }
    let p = Pair::new();
    p.a.set(Capability::Tunnel, Mode::Allow);
    let (port, _) = server(ws);
    svc(&p.a).share(port, "hmr").unwrap();
    let (_, local) = open(&p, port);
    let mut s = connect(local);
    s.write_all(b"GET /hmr HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
        .unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut status = String::new();
    r.read_line(&mut status).unwrap();
    assert!(status.starts_with("HTTP/1.1 101"));
    let mut line = String::new();
    loop {
        line.clear();
        r.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
    }
    for i in 0..3u8 {
        let mut f = [0u8; 9];
        r.read_exact(&mut f).unwrap();
        assert_eq!(&f[2..8], b"reload");
        assert_eq!(f[8], b'0' + i);
    }
    s.write_all(&[0x81, 2, b'o', b'k']).unwrap();
    let mut f = [0u8; 4];
    r.read_exact(&mut f).unwrap();
    assert_eq!(&f, &[0x81, 2, b'o', b'k']);
}

/// HTTP/2 with prior knowledge: the client preface and frames pass through
/// untouched (the tunnel is byte-transparent TCP).
#[test]
fn http2_preface_and_frames_pass_through() {
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    fn h2(mut s: TcpStream) {
        let mut p = [0u8; 24];
        s.read_exact(&mut p).unwrap();
        assert_eq!(&p, PREFACE);
        // Client SETTINGS (empty).
        let mut f = [0u8; 9];
        s.read_exact(&mut f).unwrap();
        // Server SETTINGS, then SETTINGS ACK.
        s.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).unwrap();
        s.write_all(&[0, 0, 0, 4, 1, 0, 0, 0, 0]).unwrap();
    }
    let p = Pair::new();
    p.a.set(Capability::Tunnel, Mode::Allow);
    let (port, _) = server(h2);
    svc(&p.a).share(port, "").unwrap();
    let (_, local) = open(&p, port);
    let mut s = connect(local);
    s.write_all(PREFACE).unwrap();
    s.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).unwrap();
    let mut f = [0u8; 18];
    s.read_exact(&mut f).unwrap();
    assert_eq!(f[3], 4);
    assert_eq!(f[4], 0);
    assert_eq!(f[12], 4);
    assert_eq!(f[13], 1);
}

#[test]
fn lan_exposure_needs_confirmation_for_always_on() {
    let (p, port) = shared_pair();
    let a = svc(&p.a);
    assert!(a.set_lan(port, LanMode::Always, false).is_err());
    assert!(a.lan_exposed().is_empty());
    // Unknown ports can't be exposed.
    assert!(a.set_lan(1, LanMode::OneHour, true).is_err());
    if urdevice_core::node::local_ipv4_addrs().is_empty() {
        return; // No LAN address on this machine.
    }
    a.set_lan(port, LanMode::Always, true).unwrap();
    let v = a.shared().into_iter().find(|v| v.port == port).unwrap();
    assert_eq!(v.lan, LanMode::Always);
    assert!(v.lan_listening);
    assert_eq!(a.lan_exposed(), vec![port]);
    // Reachable on the LAN address, forwarded to loopback.
    let addr = v.lan_addrs[0];
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(WAIT))).unwrap();
    s.write_all(b"lan\n").unwrap();
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    assert_eq!(line, "lan\n");

    a.set_lan(port, LanMode::OneHour, false).unwrap();
    let v = a.shared().into_iter().find(|v| v.port == port).unwrap();
    assert_eq!(v.lan, LanMode::OneHour);
    assert!(v.lan_until > 0);
    a.set_lan(port, LanMode::Off, false).unwrap();
    assert!(a.lan_exposed().is_empty());
    assert!(TcpStream::connect(addr).is_err());
}
