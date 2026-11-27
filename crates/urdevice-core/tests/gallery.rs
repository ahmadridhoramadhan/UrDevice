//! Gallery browsing and download between a desktop and an in-process
//! Android node (R8.7, design §9.2).

#![allow(clippy::unwrap_used)]

mod harness;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use harness::{Dev, Pair, WAIT, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::files::{Files, FilesHost, TransferState};
use urdevice_core::services::gallery::{
    Filter, Gallery, GalleryEvents, GalleryHost, HostPage, Query,
};
use urdevice_core::{CoreError, Result};
use urdevice_proto::features::gallery::{GalleryAlbum, GalleryAlbums, GalleryItem, GalleryPage};

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
    fn page(&self, cursor: u64, limit: u32, _filter: &Filter) -> Result<HostPage> {
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

    fn albums(&self) -> Result<Vec<GalleryAlbum>> {
        Ok(vec![
            GalleryAlbum {
                id: -5,
                name: "Camera".into(),
                count: 3,
            },
            GalleryAlbum {
                id: 9,
                name: "n".repeat(300),
                count: 1,
            },
        ])
    }

    fn preview(&self, id: u64, max_side: u32) -> Result<Vec<u8>> {
        // The host never makes more than the largest side the protocol allows.
        assert!(max_side <= 2048);
        if id == 99 {
            return Err(CoreError::Refused("failed".into()));
        }
        Ok(vec![0xff, 0xd8, 0xff, id as u8])
    }

    fn open(&self, id: u64) -> Result<(std::fs::File, u64)> {
        let p = self.dir.join(id.to_string());
        let b = Self::bytes(id);
        std::fs::write(&p, &b)?;
        Ok((std::fs::File::open(&p)?, b.len() as u64))
    }
}

#[derive(Default)]
struct Pages {
    pages: Mutex<Vec<GalleryPage>>,
    albums: Mutex<Vec<GalleryAlbums>>,
    previews: Mutex<Vec<(u64, Option<Vec<u8>>)>>,
}
impl GalleryEvents for Pages {
    fn page(&self, _peer: DeviceId, page: &GalleryPage) {
        self.pages.lock().unwrap().push(page.clone());
    }
    fn albums(&self, _peer: DeviceId, albums: &GalleryAlbums) {
        self.albums.lock().unwrap().push(albums.clone());
    }
    fn preview(&self, _peer: DeviceId, id: u64, jpeg: Option<Vec<u8>>) {
        self.previews.lock().unwrap().push((id, jpeg));
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
    wait_until("page 1", WAIT, || pages.pages.lock().unwrap().len() == 1);
    let first = pages.pages.lock().unwrap()[0].clone();
    assert_eq!(first.items.len(), 60);
    assert_eq!(first.next_cursor, 60);
    desk.browse(p.b.id(), first.next_cursor).unwrap();
    wait_until("page 2", WAIT, || pages.pages.lock().unwrap().len() == 2);
    let second = pages.pages.lock().unwrap()[1].clone();
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
    wait_until("error", WAIT, || pages.pages.lock().unwrap().len() == 1);
    assert_eq!(pages.pages.lock().unwrap()[0].error, "permission");

    // Declined on the phone.
    desk.close(p.b.id());
    desk.browse(p.b.id(), 0).unwrap();
    p.b.answer_prompt(false);
    wait_until("declined", WAIT, || pages.pages.lock().unwrap().len() == 2);
    assert_eq!(pages.pages.lock().unwrap()[1].error, "declined");

    // A Linux peer has no gallery: the request is refused.
    let laptop = p.b.node.service::<Gallery>().unwrap();
    let lp = Arc::new(Pages::default());
    laptop.events.set(lp.clone());
    laptop.browse(p.a.id(), 0).unwrap();
    wait_until("unsupported", WAIT, || lp.pages.lock().unwrap().len() == 1);
    assert_eq!(lp.pages.lock().unwrap()[0].error, "unsupported");
    std::fs::remove_dir_all(&store).unwrap();
}

#[test]
fn albums_previews_and_tags_work() {
    let p = pair();
    let store = temp("phone");
    let phone = p.b.node.service::<Gallery>().unwrap();
    phone.host.set(Arc::new(Phone {
        dir: store.clone(),
        count: 5,
        allowed: true,
    }));
    let desk = p.a.node.service::<Gallery>().unwrap();
    let ev = Arc::new(Pages::default());
    desk.events.set(ev.clone());

    // A list carries its tag back.
    desk.browse_with(
        p.b.id(),
        0,
        Query {
            filter: Filter {
                kind: 2,
                album: Some(-5),
            },
            tag: 77,
        },
    )
    .unwrap();
    assert_eq!(p.b.answer_prompt(true).capability, "gallery.v1");
    wait_until("page", WAIT, || ev.pages.lock().unwrap().len() == 1);
    assert_eq!(ev.pages.lock().unwrap()[0].tag, 77);

    // Albums: names stay short.
    desk.albums(p.b.id()).unwrap();
    wait_until("albums", WAIT, || ev.albums.lock().unwrap().len() == 1);
    let a = ev.albums.lock().unwrap()[0].clone();
    assert_eq!(a.albums.len(), 2);
    assert_eq!(a.albums[0].id, -5);
    assert!(a.albums[1].name.len() <= 64);

    // A preview arrives whole; one that cannot be made says so.
    desk.preview(p.b.id(), 3).unwrap();
    wait_until("preview", WAIT, || ev.previews.lock().unwrap().len() == 1);
    assert_eq!(
        ev.previews.lock().unwrap()[0],
        (3, Some(vec![0xff, 0xd8, 0xff, 3]))
    );
    desk.preview(p.b.id(), 99).unwrap();
    wait_until("failed preview", WAIT, || {
        ev.previews.lock().unwrap().len() == 2
    });
    assert_eq!(ev.previews.lock().unwrap()[1], (99, None));
    std::fs::remove_dir_all(&store).unwrap();
}
