//! Gallery browsing and download between a desktop and an in-process
//! Android node (R8.7, design §9.2).

#![allow(clippy::unwrap_used)]

mod harness;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use harness::{Dev, Pair, WAIT, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::files::{Files, FilesHost, TransferState};
use urdevice_core::services::gallery::{Gallery, GalleryEvents, GalleryHost, HostPage};
use urdevice_core::{CoreError, Result};
use urdevice_proto::features::gallery::{GalleryItem, GalleryPage};

struct Dir(PathBuf);
impl FilesHost for Dir {
    fn incoming_dir(&self) -> PathBuf {
        self.0.clone()
    }
}

struct Phone {
    dir: PathBuf,
    count: u64,
    allowed: bool,
}

impl Phone {
    fn bytes(id: u64) -> Vec<u8> {
        (0..(100_000 + id as usize * 1000))
            .map(|i| (i as u64 * id % 256) as u8)
            .collect()
    }
}

impl GalleryHost for Phone {
    fn page(&self, cursor: u64, limit: u32) -> Result<HostPage> {
        if !self.allowed {
            return Err(CoreError::Refused("permission".into()));
        }
        let end = (cursor + limit as u64).min(self.count);
        let items = (cursor..end)
            .map(|i| GalleryItem {
                id: i + 1,
                name: if i == 0 {
                    "../../evil.jpg".into()
                } else {
                    format!("IMG_{i}.jpg")
                },
                mime: "image/jpeg".into(),
                size: Self::bytes(i + 1).len() as u64,
                taken_ms: 0,
                duration_ms: 0,
                thumb: vec![0xff, 0xd8, 0xff],
            })
            .collect();
        Ok(HostPage {
            items,
            more: end < self.count,
        })
    }

    fn open(&self, id: u64) -> Result<(std::fs::File, u64)> {
        let p = self.dir.join(id.to_string());
        let b = Self::bytes(id);
        std::fs::write(&p, &b)?;
        Ok((std::fs::File::open(&p)?, b.len() as u64))
    }
}

#[derive(Default)]
struct Pages(Mutex<Vec<GalleryPage>>);
impl GalleryEvents for Pages {
    fn page(&self, _peer: DeviceId, page: &GalleryPage) {
        self.0.lock().unwrap().push(page.clone());
    }
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "urdevice-gallery-it-{tag}-{}-{}",
        std::process::id(),
        urdevice_core::crypto::random_u64().unwrap()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pair() -> Pair {
    Pair::with(
        Dev::with("Laptop", "linux", vec![]),
        Dev::with("Phone", "android", vec![]),
    )
}

#[test]
fn desktop_browses_pages_and_downloads_items() {
    let p = pair();
    let store = temp("phone");
    let dst = temp("dst");
    let phone = p.b.node.service::<Gallery>().unwrap();
    phone.host.set(Arc::new(Phone {
        dir: store.clone(),
        count: 70,
        allowed: true,
    }));
    let desk = p.a.node.service::<Gallery>().unwrap();
    let pages = Arc::new(Pages::default());
    desk.events.set(pages.clone());
    p.a.node
        .service::<Files>()
        .unwrap()
        .set_host(Arc::new(Dir(dst.clone())));

    desk.browse(p.b.id(), 0).unwrap();
    // Gallery is Always Ask; one approval covers the browsing session.
    assert_eq!(p.b.answer_prompt(true).capability, "gallery.v1");
    wait_until("page 1", WAIT, || pages.0.lock().unwrap().len() == 1);
    let first = pages.0.lock().unwrap()[0].clone();
    assert_eq!(first.items.len(), 60);
    assert_eq!(first.next_cursor, 60);
    desk.browse(p.b.id(), first.next_cursor).unwrap();
    wait_until("page 2", WAIT, || pages.0.lock().unwrap().len() == 2);
    let second = pages.0.lock().unwrap()[1].clone();
    assert_eq!(second.items.len(), 10);
    assert_eq!(second.next_cursor, 0);

    let pick = vec![first.items[0].clone(), first.items[5].clone()];
    let handles = desk.download(p.b.id(), pick).unwrap();
    let files = p.a.node.service::<Files>().unwrap();
    wait_until("downloaded", WAIT, || {
        files
            .transfers()
            .iter()
            .any(|t| t.id == handles[0] && t.state == TransferState::Done)
    });
    // The hostile name was cleaned to stay inside the folder.
    assert_eq!(
        std::fs::read(dst.join("evil.jpg")).unwrap(),
        Phone::bytes(1)
    );
    assert_eq!(
        std::fs::read(dst.join("IMG_5.jpg")).unwrap(),
        Phone::bytes(6)
    );
    assert!(!dst.parent().unwrap().join("evil.jpg").exists());
    std::fs::remove_dir_all(&store).unwrap();
    std::fs::remove_dir_all(&dst).unwrap();
}

#[test]
fn errors_reach_the_browser() {
    let p = pair();
    let store = temp("phone");
    let phone = p.b.node.service::<Gallery>().unwrap();
    phone.host.set(Arc::new(Phone {
        dir: store.clone(),
        count: 3,
        allowed: false,
    }));
    let desk = p.a.node.service::<Gallery>().unwrap();
    let pages = Arc::new(Pages::default());
    desk.events.set(pages.clone());
    desk.browse(p.b.id(), 0).unwrap();
    p.b.answer_prompt(true);
    wait_until("error", WAIT, || pages.0.lock().unwrap().len() == 1);
    assert_eq!(pages.0.lock().unwrap()[0].error, "permission");

    // Declined on the phone.
    desk.close(p.b.id());
    desk.browse(p.b.id(), 0).unwrap();
    p.b.answer_prompt(false);
    wait_until("declined", WAIT, || pages.0.lock().unwrap().len() == 2);
    assert_eq!(pages.0.lock().unwrap()[1].error, "declined");

    // A Linux peer has no gallery: the request is refused.
    let laptop = p.b.node.service::<Gallery>().unwrap();
    let lp = Arc::new(Pages::default());
    laptop.events.set(lp.clone());
    laptop.browse(p.a.id(), 0).unwrap();
    wait_until("unsupported", WAIT, || lp.0.lock().unwrap().len() == 1);
    assert_eq!(lp.0.lock().unwrap()[0].error, "unsupported");
    std::fs::remove_dir_all(&store).unwrap();
}
