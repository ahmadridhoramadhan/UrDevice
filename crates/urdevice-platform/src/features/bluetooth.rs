//! Bluetooth fallback (#23): the GATT central on desktops (design §8.2).
//!
//! `btleplug` drives BlueZ on Linux, WinRT on Windows, and CoreBluetooth on
//! macOS. The central scans for the UrDevice service, connects, subscribes
//! to the phone's notify characteristic, and moves raw packets between the
//! radio and the core. Everything it carries is already Noise-encrypted by
//! the core; this module never sees plaintext, and logs no packet content.

use std::sync::Arc;
use std::time::Duration;

use btleplug::api::{
    Central as _, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures_util::StreamExt;
use tokio::sync::{OnceCell, mpsc};
use urdevice_core::platform::BoxFuture;
use urdevice_core::services::bluetooth::{BleCentral, GATT_RX, GATT_SERVICE, GATT_TX, RadioLink};
use urdevice_core::{CoreError, Result};
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Default ATT MTU when the platform does not report one.
const DEFAULT_MTU: u16 = 23;
/// Retries of one write-without-response while the radio is busy.
const WRITE_RETRIES: u32 = 20;

fn ble_err(e: btleplug::Error) -> CoreError {
    CoreError::Network(format!("bluetooth: {e}"))
}

fn uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|_| CoreError::InvalidInput("bad uuid".into()))
}

struct Radio {
    // The manager owns the platform session; keep it alive with the adapter.
    _manager: Manager,
    adapter: Adapter,
}

/// The desktop's GATT central. Cheap to create: the radio is opened on the
/// first connection attempt, so a computer without Bluetooth costs nothing.
#[derive(Default)]
pub struct BtleCentral {
    radio: Arc<OnceCell<Radio>>,
}

/// The central for this desktop.
pub fn central() -> Arc<dyn BleCentral> {
    Arc::new(BtleCentral::default())
}

impl BleCentral for BtleCentral {
    fn connect(
        &self,
        scan: Duration,
        skip: Vec<String>,
    ) -> BoxFuture<'static, Result<(String, RadioLink)>> {
        let radio = self.radio.clone();
        Box::pin(async move {
            let radio = radio
                .get_or_try_init(|| async {
                    let manager = Manager::new().await.map_err(ble_err)?;
                    let adapter = manager
                        .adapters()
                        .await
                        .map_err(ble_err)?
                        .into_iter()
                        .next()
                        .ok_or_else(|| CoreError::unsupported("bluetooth", "no adapter"))?;
                    Ok::<_, CoreError>(Radio {
                        _manager: manager,
                        adapter,
                    })
                })
                .await?;
            let p = find(&radio.adapter, scan, &skip).await?;
            let id = p.id().to_string();
            let link = open(p).await?;
            Ok((id, link))
        })
    }
}

async fn advertises(p: &Peripheral, service: Uuid) -> bool {
    matches!(p.properties().await, Ok(Some(props)) if props.services.contains(&service))
}

/// Scan until a peripheral with the UrDevice service shows up.
async fn find(adapter: &Adapter, scan: Duration, skip: &[String]) -> Result<Peripheral> {
    let service = uuid(GATT_SERVICE)?;
    let mut events = adapter.events().await.map_err(ble_err)?;
    adapter
        .start_scan(ScanFilter {
            services: vec![service],
        })
        .await
        .map_err(ble_err)?;
    let found = tokio::time::timeout(scan, async {
        // Already known from an earlier scan.
        for p in adapter.peripherals().await.map_err(ble_err)? {
            if !skip.contains(&p.id().to_string()) && advertises(&p, service).await {
                return Ok(p);
            }
        }
        while let Some(e) = events.next().await {
            let id = match e {
                CentralEvent::DeviceDiscovered(id)
                | CentralEvent::DeviceUpdated(id)
                | CentralEvent::ServicesAdvertisement { id, .. } => id,
                _ => continue,
            };
            if skip.contains(&id.to_string()) {
                continue;
            }
            if let Ok(p) = adapter.peripheral(&id).await
                && advertises(&p, service).await
            {
                return Ok(p);
            }
        }
        Err(CoreError::Network("bluetooth: scan ended".into()))
    })
    .await;
    let _ = adapter.stop_scan().await;
    found.map_err(|_| CoreError::Network("no UrDevice phone in Bluetooth range".into()))?
}

fn characteristic(p: &Peripheral, id: Uuid) -> Result<Characteristic> {
    p.characteristics()
        .into_iter()
        .find(|c| c.uuid == id)
        .ok_or_else(|| CoreError::Protocol("UrDevice GATT characteristic missing".into()))
}

/// Connect, subscribe, and start the two pumps between radio and core.
async fn open(p: Peripheral) -> Result<RadioLink> {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        if !p.is_connected().await.unwrap_or(false) {
            p.connect().await.map_err(ble_err)?;
        }
        p.discover_services().await.map_err(ble_err)
    })
    .await
    .map_err(|_| CoreError::Network("bluetooth: connect timed out".into()))??;
    let rx_char = characteristic(&p, uuid(GATT_RX)?)?;
    let tx_char = characteristic(&p, uuid(GATT_TX)?)?;
    p.subscribe(&tx_char).await.map_err(ble_err)?;
    let mut notes = p.notifications().await.map_err(ble_err)?;
    let mtu = match p.mtu() {
        0 => DEFAULT_MTU,
        m => m,
    };
    let packet = usize::from(mtu).saturating_sub(3);
    tracing::info!(mtu, "Bluetooth LE link open");

    let (core_tx, mut to_radio) = mpsc::channel::<Vec<u8>>(64);
    let (from_radio, core_rx) = mpsc::channel::<Vec<u8>>(64);
    let tx_uuid = tx_char.uuid;

    // Radio to core: notifications on the phone's TX characteristic.
    let reader = tokio::spawn(async move {
        while let Some(n) = notes.next().await {
            if n.uuid == tx_uuid && from_radio.send(n.value).await.is_err() {
                break;
            }
        }
    });
    // Core to radio: writes without response. The core dropping its sender
    // ends the link.
    tokio::spawn(async move {
        'packets: while let Some(pkt) = to_radio.recv().await {
            let mut tries = 0;
            loop {
                match p.write(&rx_char, &pkt, WriteType::WithoutResponse).await {
                    Ok(()) => break,
                    Err(e) if tries < WRITE_RETRIES => {
                        tries += 1;
                        tracing::debug!(error = %e, "Bluetooth LE write busy; retrying");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(e) => {
                        tracing::info!(error = %e, "Bluetooth LE write failed");
                        break 'packets;
                    }
                }
            }
        }
        reader.abort();
        let _ = p.disconnect().await;
        tracing::info!("Bluetooth LE link closed");
    });
    Ok(RadioLink {
        tx: core_tx,
        rx: core_rx,
        packet,
    })
}
