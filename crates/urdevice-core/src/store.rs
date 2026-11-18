//! Trust store (design §4.2).
//!
//! SQLite in the app data directory. It holds no secrets, but every row that
//! decides trust carries an HMAC keyed by a secret from secure storage, so a
//! local process that edits the file can't add a paired device or flip a
//! permission to Allow. Rows with a bad MAC are ignored: a tampered paired
//! device is not trusted, and a tampered permission reads as Deny.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::crypto::{hmac_sha256, hmac_verify};
use crate::identity::DeviceId;
use crate::permission::{Capability, Mode};
use crate::{CoreError, Result};

/// How long activity-log entries are kept (R5.6).
pub const ACTIVITY_RETENTION_SECS: u64 = 30 * 24 * 3600;

/// A paired device as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedDevice {
    /// Device ID.
    pub id: DeviceId,
    /// Display name.
    pub name: String,
    /// Platform string.
    pub platform: String,
    /// Unix seconds.
    pub paired_at: u64,
    /// Unix seconds, 0 when never seen.
    pub last_seen: u64,
    /// Addresses the device was last reached at.
    pub last_addrs: Vec<SocketAddr>,
    /// False after the user chose Disconnect (R4.7).
    pub auto_connect: bool,
    /// Name of the device whose user chose Disconnect, when `auto_connect` is false.
    pub disconnected_by: Option<String>,
}

/// One activity-log entry (R5.6). Never holds content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityLogEntry {
    /// Device that asked.
    pub device_name: String,
    /// Capability string.
    pub capability: String,
    /// `allowed`, `denied`, `timeout`, `declined`, or `stopped`.
    pub decision: String,
    /// Unix seconds.
    pub ts: u64,
}

/// A shared localhost port (R13, used from Phase 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedPort {
    /// TCP port on loopback.
    pub port: u16,
    /// User label.
    pub label: String,
    /// `None` = not exposed, `Some(0)` = always on, otherwise a Unix time.
    pub lan_expose_until: Option<u64>,
}

/// The SQLite trust store.
pub struct TrustStore {
    db: Connection,
    mac_key: Vec<u8>,
}

impl std::fmt::Debug for TrustStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustStore").finish_non_exhaustive()
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS paired_device(
  device_id        BLOB PRIMARY KEY,
  cert_fingerprint BLOB NOT NULL,
  ble_static_pub   BLOB,
  name             TEXT NOT NULL,
  platform         TEXT NOT NULL,
  paired_at        INTEGER NOT NULL,
  last_seen        INTEGER NOT NULL DEFAULT 0,
  last_addrs       TEXT NOT NULL DEFAULT '',
  auto_connect     INTEGER NOT NULL DEFAULT 1,
  disconnected_by  TEXT,
  row_mac          BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS permission(
  capability TEXT PRIMARY KEY,
  mode       TEXT NOT NULL,
  local_only INTEGER NOT NULL DEFAULT 0,
  row_mac    BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS activity_log(
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id   BLOB NOT NULL,
  device_name TEXT NOT NULL,
  capability  TEXT NOT NULL,
  decision    TEXT NOT NULL,
  ts          INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS shared_port(
  port             INTEGER PRIMARY KEY,
  label            TEXT NOT NULL,
  lan_expose_until INTEGER,
  row_mac          BLOB NOT NULL
);
"#;

/// Length-prefixed concatenation, so field boundaries can't be shifted.
fn mac_input(table: &str, fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in std::iter::once(table.as_bytes()).chain(fields.iter().copied()) {
        out.extend_from_slice(&(f.len() as u32).to_be_bytes());
        out.extend_from_slice(f);
    }
    out
}

fn addrs_to_text(addrs: &[SocketAddr]) -> String {
    addrs
        .iter()
        .map(|a| a.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn addrs_from_text(s: &str) -> Vec<SocketAddr> {
    s.split(',')
        .filter_map(|a| a.parse().ok())
        .take(8)
        .collect()
}

fn i64_of(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn u64_of(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

impl TrustStore {
    /// Open or create the store at `path`. `mac_key` comes from secure storage.
    pub fn open(path: &Path, mac_key: Vec<u8>) -> Result<Self> {
        let db = Connection::open(path)?;
        restrict_permissions(path);
        Self::init(db, mac_key)
    }

    /// An in-memory store for tests.
    pub fn open_in_memory(mac_key: Vec<u8>) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, mac_key)
    }

    fn init(db: Connection, mac_key: Vec<u8>) -> Result<Self> {
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        db.execute_batch(SCHEMA)?;
        Ok(Self { db, mac_key })
    }

    // -- meta ----------------------------------------------------------------

    /// Read a meta value.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    /// Write a meta value.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // -- paired devices ------------------------------------------------------

    fn device_mac_input(d: &PairedDevice) -> Vec<u8> {
        let auto = [u8::from(d.auto_connect)];
        let paired_at = d.paired_at.to_be_bytes();
        mac_input(
            "paired_device",
            &[
                &d.id.0,
                d.name.as_bytes(),
                d.platform.as_bytes(),
                &paired_at,
                &auto,
                d.disconnected_by.as_deref().unwrap_or("").as_bytes(),
            ],
        )
    }

    fn device_mac(&self, d: &PairedDevice) -> [u8; 32] {
        hmac_sha256(&self.mac_key, &[&Self::device_mac_input(d)])
    }

    fn device_mac_ok(&self, d: &PairedDevice, mac: &[u8]) -> bool {
        hmac_verify(&self.mac_key, &[&Self::device_mac_input(d)], mac)
    }

    /// All paired devices with a valid MAC.
    pub fn paired_devices(&self) -> Result<Vec<PairedDevice>> {
        let mut stmt = self.db.prepare(
            "SELECT device_id, name, platform, paired_at, last_seen, last_addrs, auto_connect,
                    disconnected_by, row_mac
             FROM paired_device ORDER BY paired_at",
        )?;
        let rows = stmt.query_map([], |r| {
            let id: Vec<u8> = r.get(0)?;
            Ok((
                id,
                PairedDevice {
                    id: DeviceId([0; 32]),
                    name: r.get(1)?,
                    platform: r.get(2)?,
                    paired_at: u64_of(r.get(3)?),
                    last_seen: u64_of(r.get(4)?),
                    last_addrs: addrs_from_text(&r.get::<_, String>(5)?),
                    auto_connect: r.get::<_, i64>(6)? != 0,
                    disconnected_by: r.get(7)?,
                },
                r.get::<_, Vec<u8>>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, mut d, mac) = row?;
            let Some(id) = DeviceId::from_slice(&id) else {
                continue;
            };
            d.id = id;
            if self.device_mac_ok(&d, &mac) {
                out.push(d);
            } else {
                tracing::warn!(peer = %id.short_hex(), "paired device row failed its MAC; ignoring it");
            }
        }
        Ok(out)
    }

    /// One paired device, if stored and valid.
    pub fn paired(&self, id: DeviceId) -> Result<Option<PairedDevice>> {
        Ok(self.paired_devices()?.into_iter().find(|d| d.id == id))
    }

    /// Add or replace a paired device.
    pub fn upsert_paired(&self, d: &PairedDevice) -> Result<()> {
        let mac = self.device_mac(d);
        self.db.execute(
            "INSERT INTO paired_device(device_id, cert_fingerprint, name, platform, paired_at,
                                       last_seen, last_addrs, auto_connect, disconnected_by, row_mac)
             VALUES(?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(device_id) DO UPDATE SET
               name = excluded.name, platform = excluded.platform, paired_at = excluded.paired_at,
               last_seen = excluded.last_seen, last_addrs = excluded.last_addrs,
               auto_connect = excluded.auto_connect, disconnected_by = excluded.disconnected_by,
               row_mac = excluded.row_mac",
            params![
                &d.id.0[..],
                d.name,
                d.platform,
                i64_of(d.paired_at),
                i64_of(d.last_seen),
                addrs_to_text(&d.last_addrs),
                i64::from(d.auto_connect),
                d.disconnected_by,
                &mac[..],
            ],
        )?;
        Ok(())
    }

    /// Update `last_seen` and `last_addrs`. These two fields are hints, not
    /// trust decisions, so they are outside the MAC.
    pub fn touch(&self, id: DeviceId, last_seen: u64, addrs: &[SocketAddr]) -> Result<()> {
        self.db.execute(
            "UPDATE paired_device SET last_seen = ?2, last_addrs = ?3 WHERE device_id = ?1",
            params![&id.0[..], i64_of(last_seen), addrs_to_text(addrs)],
        )?;
        Ok(())
    }

    /// Delete a paired device (R3.9).
    pub fn remove_paired(&self, id: DeviceId) -> Result<bool> {
        let n = self.db.execute(
            "DELETE FROM paired_device WHERE device_id = ?1",
            [&id.0[..]],
        )?;
        Ok(n > 0)
    }

    /// Delete every paired device (R1.5).
    pub fn remove_all_paired(&self) -> Result<()> {
        self.db.execute("DELETE FROM paired_device", [])?;
        Ok(())
    }

    // -- permissions ---------------------------------------------------------

    fn permission_mac(&self, cap: &str, mode: &str, local_only: bool) -> [u8; 32] {
        let lo = [u8::from(local_only)];
        let input = mac_input("permission", &[cap.as_bytes(), mode.as_bytes(), &lo]);
        hmac_sha256(&self.mac_key, &[&input])
    }

    /// The stored modes. A missing or tampered row is absent from the map.
    pub fn permissions(&self) -> Result<HashMap<Capability, Mode>> {
        let mut stmt = self
            .db
            .prepare("SELECT capability, mode, local_only, row_mac FROM permission")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (cap_s, mode_s, lo, mac) = row?;
            let lo_b = [u8::from(lo)];
            let input = mac_input("permission", &[cap_s.as_bytes(), mode_s.as_bytes(), &lo_b]);
            if !hmac_verify(&self.mac_key, &[&input], &mac) {
                tracing::warn!(cap = %cap_s, "permission row failed its MAC; reading it as Deny");
                continue;
            }
            if let (Ok(cap), Some(mode)) = (cap_s.parse::<Capability>(), Mode::parse(&mode_s)) {
                out.insert(cap, mode);
            }
        }
        Ok(out)
    }

    /// Set one mode.
    pub fn set_permission(&self, cap: Capability, mode: Mode) -> Result<()> {
        let mac = self.permission_mac(cap.as_str(), mode.as_str(), cap.local_only());
        self.db.execute(
            "INSERT INTO permission(capability, mode, local_only, row_mac) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(capability) DO UPDATE SET mode = excluded.mode,
               local_only = excluded.local_only, row_mac = excluded.row_mac",
            params![
                cap.as_str(),
                mode.as_str(),
                i64::from(cap.local_only()),
                &mac[..]
            ],
        )?;
        Ok(())
    }

    // -- activity log --------------------------------------------------------

    /// Record a decision (R5.6). `now` is Unix seconds.
    pub fn log_activity(
        &self,
        device: DeviceId,
        device_name: &str,
        cap: &str,
        decision: &str,
        now: u64,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO activity_log(device_id, device_name, capability, decision, ts)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![&device.0[..], device_name, cap, decision, i64_of(now)],
        )?;
        Ok(())
    }

    /// The newest entries first.
    pub fn activity_log(&self, limit: usize) -> Result<Vec<ActivityLogEntry>> {
        let mut stmt = self.db.prepare(
            "SELECT device_name, capability, decision, ts FROM activity_log
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(1000)], |r| {
            Ok(ActivityLogEntry {
                device_name: r.get(0)?,
                capability: r.get(1)?,
                decision: r.get(2)?,
                ts: u64_of(r.get(3)?),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Delete entries older than 30 days.
    pub fn prune_activity(&self, now: u64) -> Result<usize> {
        let cutoff = now.saturating_sub(ACTIVITY_RETENTION_SECS);
        Ok(self
            .db
            .execute("DELETE FROM activity_log WHERE ts < ?1", [i64_of(cutoff)])?)
    }

    // -- shared ports --------------------------------------------------------

    fn port_mac_input(p: &SharedPort) -> Vec<u8> {
        let port = p.port.to_be_bytes();
        let until = p
            .lan_expose_until
            .map(|u| u.to_be_bytes().to_vec())
            .unwrap_or_default();
        mac_input("shared_port", &[&port, p.label.as_bytes(), &until])
    }

    fn port_mac(&self, p: &SharedPort) -> [u8; 32] {
        hmac_sha256(&self.mac_key, &[&Self::port_mac_input(p)])
    }

    /// Shared ports with a valid MAC.
    pub fn shared_ports(&self) -> Result<Vec<SharedPort>> {
        let mut stmt = self.db.prepare(
            "SELECT port, label, lan_expose_until, row_mac FROM shared_port ORDER BY port",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                SharedPort {
                    port: u16::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                    label: r.get(1)?,
                    lan_expose_until: r.get::<_, Option<i64>>(2)?.map(u64_of),
                },
                r.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (p, mac) = row?;
            if p.port != 0 && hmac_verify(&self.mac_key, &[&Self::port_mac_input(&p)], &mac) {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Add or replace a shared port.
    pub fn upsert_shared_port(&self, p: &SharedPort) -> Result<()> {
        if p.port == 0 {
            return Err(CoreError::InvalidInput("port 0".into()));
        }
        let mac = self.port_mac(p);
        self.db.execute(
            "INSERT INTO shared_port(port, label, lan_expose_until, row_mac) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(port) DO UPDATE SET label = excluded.label,
               lan_expose_until = excluded.lan_expose_until, row_mac = excluded.row_mac",
            params![
                i64::from(p.port),
                p.label,
                p.lan_expose_until.map(i64_of),
                &mac[..]
            ],
        )?;
        Ok(())
    }

    /// Stop sharing a port.
    pub fn remove_shared_port(&self, port: u16) -> Result<()> {
        self.db
            .execute("DELETE FROM shared_port WHERE port = ?1", [i64::from(port)])?;
        Ok(())
    }

    /// Delete everything: used by "Delete all data".
    pub fn wipe(&self) -> Result<()> {
        self.db.execute_batch(
            "DELETE FROM paired_device; DELETE FROM permission; DELETE FROM activity_log;
             DELETE FROM shared_port; DELETE FROM meta;",
        )?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(error = %e, "could not set trust store permissions to 0600");
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(n: u8) -> PairedDevice {
        PairedDevice {
            id: DeviceId([n; 32]),
            name: format!("dev{n}"),
            platform: "linux".into(),
            paired_at: 100,
            last_seen: 0,
            last_addrs: vec!["192.168.1.2:47811".parse().unwrap()],
            auto_connect: true,
            disconnected_by: None,
        }
    }

    #[test]
    fn paired_device_crud() {
        let s = TrustStore::open_in_memory(vec![1; 32]).unwrap();
        s.upsert_paired(&device(1)).unwrap();
        s.upsert_paired(&device(2)).unwrap();
        assert_eq!(s.paired_devices().unwrap().len(), 2);
        let mut d = device(1);
        d.auto_connect = false;
        d.disconnected_by = Some("Pixel".into());
        s.upsert_paired(&d).unwrap();
        assert_eq!(s.paired(d.id).unwrap(), Some(d.clone()));
        s.touch(d.id, 500, &[]).unwrap();
        assert_eq!(s.paired(d.id).unwrap().unwrap().last_seen, 500);
        assert!(s.remove_paired(d.id).unwrap());
        assert_eq!(s.paired(d.id).unwrap(), None);
    }

    #[test]
    fn tampered_rows_are_ignored() {
        let s = TrustStore::open_in_memory(vec![1; 32]).unwrap();
        s.upsert_paired(&device(1)).unwrap();
        s.set_permission(Capability::Input, Mode::Ask).unwrap();
        // Another process flips trust-relevant columns without the key.
        s.db.execute("UPDATE paired_device SET auto_connect = 0", [])
            .unwrap();
        s.db.execute("UPDATE permission SET mode = 'allow'", [])
            .unwrap();
        assert!(s.paired_devices().unwrap().is_empty());
        assert!(!s.permissions().unwrap().contains_key(&Capability::Input));
    }

    #[test]
    fn a_different_key_trusts_nothing() {
        let s = TrustStore::open_in_memory(vec![1; 32]).unwrap();
        s.upsert_paired(&device(1)).unwrap();
        let s2 = TrustStore {
            db: s.db,
            mac_key: vec![2; 32],
        };
        assert!(s2.paired_devices().unwrap().is_empty());
    }

    #[test]
    fn activity_log_prunes_after_30_days() {
        let s = TrustStore::open_in_memory(vec![1; 32]).unwrap();
        let id = DeviceId([1; 32]);
        s.log_activity(id, "Pixel", "input.v1", "allowed", 1_000)
            .unwrap();
        s.log_activity(
            id,
            "Pixel",
            "input.v1",
            "denied",
            ACTIVITY_RETENTION_SECS + 5_000,
        )
        .unwrap();
        assert_eq!(
            s.prune_activity(ACTIVITY_RETENTION_SECS + 5_000).unwrap(),
            1
        );
        let log = s.activity_log(10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].decision, "denied");
    }

    #[test]
    fn shared_ports() {
        let s = TrustStore::open_in_memory(vec![1; 32]).unwrap();
        let p = SharedPort {
            port: 3000,
            label: "vite".into(),
            lan_expose_until: None,
        };
        s.upsert_shared_port(&p).unwrap();
        assert_eq!(s.shared_ports().unwrap(), vec![p]);
        s.remove_shared_port(3000).unwrap();
        assert!(s.shared_ports().unwrap().is_empty());
        assert!(
            s.upsert_shared_port(&SharedPort {
                port: 0,
                label: String::new(),
                lan_expose_until: None
            })
            .is_err()
        );
    }
}
