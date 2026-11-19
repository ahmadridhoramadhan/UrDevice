//! File transfer end to end between two in-process nodes (R8, design §9.2).

#![allow(clippy::unwrap_used)]

mod harness;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use harness::{Pair, WAIT, wait_until};
use urdevice_core::services::files::{Files, FilesHost, Source, TransferState, TransferView};
use urdevice_core::{Capability, Mode};

struct Dir(PathBuf);

impl FilesHost for Dir {
    fn incoming_dir(&self) -> PathBuf {
        self.0.clone()
    }
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "urdevice-files-it-{tag}-{}-{}",
        std::process::id(),
        urdevice_core::crypto::random_u64().unwrap()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn source(dir: &Path, rel: &str, bytes: &[u8]) -> Source {
    let p = dir.join(rel.replace('/', "_"));
    std::fs::write(&p, bytes).unwrap();
    Source {
        rel_path: rel.into(),
        size: bytes.len() as u64,
        open: Arc::new(move || std::fs::File::open(&p)),
    }
}

fn view(f: &Files, handle: u64) -> TransferView {
    f.transfers().into_iter().find(|t| t.id == handle).unwrap()
}

fn setup() -> (Pair, Arc<Files>, Arc<Files>, PathBuf, PathBuf) {
    let p = Pair::new();
    let a = p.a.node.service::<Files>().unwrap();
    let b = p.b.node.service::<Files>().unwrap();
    let src = temp("src");
    let dst = temp("dst");
    b.set_host(Arc::new(Dir(dst.clone())));
    (p, a, b, src, dst)
}

#[test]
fn files_and_folders_arrive_after_approval() {
    let (p, a, b, src, dst) = setup();
    let big = data(3 * 1024 * 1024 + 17);
    let files = vec![
        source(&src, "hello.txt", b"hi"),
        source(&src, "Album/2024/big.bin", &big),
    ];
    let h = a.send(p.b.id(), files).unwrap();
    // Files is Always Ask by default: the receiver approves (R8.2).
    let prompt = p.b.answer_prompt(true);
    assert_eq!(prompt.capability, "files.v1");
    wait_until("done", WAIT, || view(&a, h).state == TransferState::Done);
    assert_eq!(std::fs::read(dst.join("hello.txt")).unwrap(), b"hi");
    assert_eq!(std::fs::read(dst.join("Album/2024/big.bin")).unwrap(), big);
    let v = view(&a, h);
    assert_eq!(v.done, v.total);
    wait_until("receiver done", WAIT, || {
        b.transfers()
            .iter()
            .any(|t| !t.outgoing && t.state == TransferState::Done && t.saved.len() == 2)
    });
    std::fs::remove_dir_all(&src).unwrap();
    std::fs::remove_dir_all(&dst).unwrap();
}

#[test]
fn a_declined_prompt_and_deny_send_nothing() {
    let (p, a, _b, src, dst) = setup();
    let h = a.send(p.b.id(), vec![source(&src, "x.txt", b"x")]).unwrap();
    p.b.answer_prompt(false);
    wait_until("declined", WAIT, || {
        view(&a, h).state == TransferState::Declined
    });
    assert_eq!(view(&a, h).reason, "declined");

    p.b.set(Capability::Files, Mode::Deny);
    wait_until("deny seen", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Files) == Some(Mode::Deny)
    });
    let h = a.send(p.b.id(), vec![source(&src, "y.txt", b"y")]).unwrap();
    wait_until("denied", WAIT, || {
        view(&a, h).state == TransferState::Declined
    });
    assert_eq!(view(&a, h).reason, "denied");
    assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
    std::fs::remove_dir_all(&src).unwrap();
    std::fs::remove_dir_all(&dst).unwrap();
}

#[test]
fn hostile_names_are_refused_and_collisions_renamed() {
    let (p, a, _b, src, dst) = setup();
    p.b.set(Capability::Files, Mode::Allow);
    wait_until("allow seen", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Files) == Some(Mode::Allow)
    });
    let h = a
        .send(p.b.id(), vec![source(&src, "../escape.txt", b"bad")])
        .unwrap();
    wait_until("refused", WAIT, || {
        view(&a, h).state == TransferState::Declined
    });
    assert_eq!(view(&a, h).reason, "invalid");
    assert!(!dst.parent().unwrap().join("escape.txt").exists());

    std::fs::write(dst.join("CON"), b"old").unwrap();
    std::fs::write(dst.join("_CON"), b"old").unwrap();
    let h = a.send(p.b.id(), vec![source(&src, "CON", b"new")]).unwrap();
    wait_until("done", WAIT, || view(&a, h).state == TransferState::Done);
    assert_eq!(std::fs::read(dst.join("_CON")).unwrap(), b"old");
    assert_eq!(std::fs::read(dst.join("_CON (1)")).unwrap(), b"new");
    std::fs::remove_dir_all(&src).unwrap();
    std::fs::remove_dir_all(&dst).unwrap();
}

#[test]
fn cancel_on_the_receiver_stops_both_and_deletes_parts() {
    let (p, a, b, src, dst) = setup();
    p.b.set(Capability::Files, Mode::Allow);
    wait_until("allow seen", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Files) == Some(Mode::Allow)
    });
    // Big enough to still be running when cancelled.
    let big = data(256 * 1024 * 1024);
    let h = a
        .send(p.b.id(), vec![source(&src, "huge.bin", &big)])
        .unwrap();
    wait_until("running", WAIT, || {
        b.transfers()
            .iter()
            .any(|t| !t.outgoing && t.state == TransferState::Running && t.done > 0)
    });
    let rh = b.transfers().iter().find(|t| !t.outgoing).unwrap().id;
    b.cancel(rh);
    wait_until("cancelled", WAIT, || {
        view(&a, h).state == TransferState::Cancelled
            && view(&b, rh).state == TransferState::Cancelled
    });
    assert_eq!(view(&a, h).reason, "peer");
    wait_until("parts gone", WAIT, || {
        std::fs::read_dir(&dst).unwrap().count() == 0
    });
    std::fs::remove_dir_all(&src).unwrap();
    std::fs::remove_dir_all(&dst).unwrap();
}

#[test]
fn an_interrupted_transfer_resumes_after_reconnect() {
    let (p, a, b, src, dst) = setup();
    p.b.set(Capability::Files, Mode::Allow);
    wait_until("allow seen", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Files) == Some(Mode::Allow)
    });
    let big = data(192 * 1024 * 1024);
    let h = a
        .send(p.b.id(), vec![source(&src, "resume.bin", &big)])
        .unwrap();
    wait_until("some bytes", WAIT, || {
        b.transfers()
            .iter()
            .any(|t| !t.outgoing && t.done > 8 * 1024 * 1024)
    });
    let a_id = p.a.id().hex();
    p.b.node.disconnect(&a_id).unwrap();
    wait_until("interrupted", WAIT, || {
        view(&a, h).state == TransferState::Interrupted
    });
    let rh = b.transfers().iter().find(|t| !t.outgoing).unwrap().id;
    wait_until("receiver interrupted", WAIT, || {
        view(&b, rh).state == TransferState::Interrupted
    });
    let kept = view(&b, rh).done;
    assert!(kept > 0);
    p.b.node.reconnect(&a_id).unwrap();
    wait_until("resumed and done", 60, || {
        view(&a, h).state == TransferState::Done
    });
    wait_until("receiver done", WAIT, || {
        view(&b, rh).state == TransferState::Done
    });
    assert_eq!(std::fs::read(dst.join("resume.bin")).unwrap(), big);
    let names: Vec<_> = std::fs::read_dir(&dst)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["resume.bin"]);
    std::fs::remove_dir_all(&src).unwrap();
    std::fs::remove_dir_all(&dst).unwrap();
}
