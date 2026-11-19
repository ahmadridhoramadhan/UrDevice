//! Media control end to end between two in-process nodes (design §9.4, R11).

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness::{Pair, WAIT, holds_for, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::media::{
    CmdKind, Media, MediaEvents, MediaHost, MediaSession, MediaState, PlayState, VolumeChange,
};
use urdevice_core::{Capability, Mode};

/// A host with two sessions that records what it was asked to do.
#[derive(Default)]
struct FakeHost {
    state: Mutex<MediaState>,
    cmds: Mutex<Vec<(String, CmdKind, u64)>>,
    vols: Mutex<Vec<VolumeChange>>,
    grants: Mutex<Vec<Grant>>,
}

impl FakeHost {
    fn new() -> Arc<Self> {
        let h = Arc::new(Self::default());
        *h.state.lock().unwrap() = MediaState {
            sessions: vec![
                MediaSession {
                    id: "spotify".into(),
                    app_name: "Spotify".into(),
                    title: "Song".into(),
                    artist: "Band".into(),
                    state: PlayState::Playing as i32,
                    position_ms: Some(5_000),
                    duration_ms: Some(200_000),
                    can_seek: true,
                    can_next: true,
                    can_prev: true,
                    can_play_pause: true,
                    ..MediaSession::default()
                },
                MediaSession {
                    id: "chrome".into(),
                    app_name: "Chrome".into(),
                    title: "Video".into(),
                    state: PlayState::Paused as i32,
                    can_play_pause: true,
                    ..MediaSession::default()
                },
            ],
            volume: 0.4,
            muted: false,
            can_volume: true,
            unavailable: String::new(),
        };
        h
    }

    fn edit(&self, f: impl FnOnce(&mut MediaState)) {
        f(&mut self.state.lock().unwrap());
    }
}

impl MediaHost for FakeHost {
    fn state(&self) -> MediaState {
        self.state.lock().unwrap().clone()
    }

    fn command(
        &self,
        grant: &Grant,
        session_id: &str,
        kind: CmdKind,
        seek_ms: u64,
    ) -> urdevice_core::Result<()> {
        self.grants.lock().unwrap().push(grant.clone());
        self.cmds
            .lock()
            .unwrap()
            .push((session_id.into(), kind, seek_ms));
        Ok(())
    }

    fn volume(&self, grant: &Grant, change: &VolumeChange) -> urdevice_core::Result<()> {
        self.grants.lock().unwrap().push(grant.clone());
        self.vols.lock().unwrap().push(change.clone());
        Ok(())
    }
}

#[derive(Default)]
struct Events {
    updates: Mutex<u32>,
    refused: Mutex<Vec<String>>,
}

impl MediaEvents for Events {
    fn updated(&self, _peer: DeviceId) {
        *self.updates.lock().unwrap() += 1;
    }

    fn refused(&self, _peer: DeviceId, reason: &str) {
        self.refused.lock().unwrap().push(reason.into());
    }
}

struct Rig {
    p: Pair,
    host: Arc<FakeHost>,
    events: Arc<Events>,
    /// Service on the controller (`a`).
    ctl: Arc<Media>,
}

impl Rig {
    fn new() -> Self {
        let p = Pair::new();
        let host = FakeHost::new();
        let events = Arc::new(Events::default());
        let ctl = p.a.node.service::<Media>().unwrap();
        let hostsvc = p.b.node.service::<Media>().unwrap();
        hostsvc.set_host(host.clone());
        ctl.set_events(events.clone());
        Self {
            p,
            host,
            events,
            ctl,
        }
    }

    fn peer(&self) -> DeviceId {
        self.p.b.id()
    }

    fn updates(&self) -> u32 {
        *self.events.updates.lock().unwrap()
    }

    fn state(&self) -> Option<MediaState> {
        self.ctl.state_of(self.peer())
    }
}

#[test]
fn sessions_reach_the_controller_and_commands_reach_the_host() {
    let r = Rig::new();
    assert!(r.state().is_none(), "nothing before the player is on");
    r.ctl.subscribe(r.peer(), true).unwrap();
    wait_until("first state", WAIT, || r.state().is_some());
    let st = r.state().unwrap();
    assert_eq!(st.sessions.len(), 2);
    assert_eq!(st.sessions[0].app_name, "Spotify");
    assert!(st.sessions[0].can_seek);
    assert!(!st.sessions[1].can_seek, "Chrome has no seek");
    assert!((st.volume - 0.4).abs() < 0.001);

    r.ctl
        .command(r.peer(), "spotify", CmdKind::PlayPause, 0)
        .unwrap();
    r.ctl
        .command(r.peer(), "spotify", CmdKind::Seek, 90_000)
        .unwrap();
    r.ctl.volume(r.peer(), VolumeChange::Set(0.75)).unwrap();
    r.ctl.volume(r.peer(), VolumeChange::Mute(true)).unwrap();
    wait_until("commands", WAIT, || {
        r.host.cmds.lock().unwrap().len() == 2 && r.host.vols.lock().unwrap().len() == 2
    });
    let cmds = r.host.cmds.lock().unwrap().clone();
    assert_eq!(cmds[0], ("spotify".into(), CmdKind::PlayPause, 0));
    assert_eq!(cmds[1], ("spotify".into(), CmdKind::Seek, 90_000));
    let vols = r.host.vols.lock().unwrap().clone();
    assert_eq!(vols[0], VolumeChange::Set(0.75));
    assert_eq!(vols[1], VolumeChange::Mute(true));
    // Every adapter call carried the node's grant for this peer.
    let grants = r.host.grants.lock().unwrap().clone();
    assert_eq!(grants.len(), 4);
    assert!(grants.iter().all(|g| g.peer() == r.p.a.id()
        && g.capability() == Capability::Media
        && g.activity() == 0));
}

#[test]
fn changes_are_pushed_and_the_playback_clock_is_not() {
    let r = Rig::new();
    r.ctl.subscribe(r.peer(), true).unwrap();
    wait_until("first state", WAIT, || r.state().is_some());
    let before = r.updates();
    // The player's clock moves with the state unchanged otherwise.
    r.host.edit(|s| s.sessions[0].position_ms = Some(6_000));
    holds_for("no push for a clock tick", 2500, || r.updates() == before);
    // A new track is a change.
    r.host.edit(|s| {
        s.sessions[0].title = "Another".into();
        s.sessions[0].position_ms = Some(0);
    });
    wait_until("title change", WAIT, || {
        r.state().is_some_and(|s| s.sessions[0].title == "Another")
    });
    // So is a mute done on the host itself.
    r.host.edit(|s| s.muted = true);
    wait_until("mute change", WAIT, || r.state().is_some_and(|s| s.muted));
}

#[test]
fn turning_the_player_off_stops_updates_until_it_is_turned_on() {
    let r = Rig::new();
    r.ctl.subscribe(r.peer(), true).unwrap();
    wait_until("first state", WAIT, || r.state().is_some());
    r.ctl.subscribe(r.peer(), false).unwrap();
    assert!(r.state().is_none());
    assert!(!r.ctl.is_on(r.peer()));
    // The host stops: a change while off never reaches the controller.
    std::thread::sleep(Duration::from_millis(300));
    r.host.edit(|s| s.sessions[0].title = "While off".into());
    holds_for("no state while off", 2500, || r.state().is_none());
    // On again: the host replies with the current state.
    r.ctl.subscribe(r.peer(), true).unwrap();
    wait_until("state after on", WAIT, || {
        r.state()
            .is_some_and(|s| s.sessions[0].title == "While off")
    });
}

#[test]
fn deny_on_the_host_refuses_commands_and_stops_pushes() {
    let r = Rig::new();
    r.ctl.subscribe(r.peer(), true).unwrap();
    wait_until("first state", WAIT, || r.state().is_some());
    r.p.b.set(Capability::Media, Mode::Deny);
    wait_until("controller sees deny", WAIT, || {
        r.p.a.remote_mode(&r.p.b, Capability::Media) == Some(Mode::Deny)
    });
    r.ctl
        .command(r.peer(), "spotify", CmdKind::Next, 0)
        .unwrap();
    wait_until("refusal", WAIT, || {
        r.events
            .refused
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == "denied")
    });
    assert!(r.host.cmds.lock().unwrap().is_empty());
    // A change on the host no longer reaches the controller.
    r.host.edit(|s| s.sessions[0].title = "After deny".into());
    holds_for("no push after deny", 2500, || {
        r.state()
            .is_none_or(|s| s.sessions[0].title != "After deny")
    });
}

#[test]
fn always_ask_needs_one_approval_and_then_works() {
    let r = Rig::new();
    r.p.b.set(Capability::Media, Mode::Ask);
    wait_until("controller sees ask", WAIT, || {
        r.p.a.remote_mode(&r.p.b, Capability::Media) == Some(Mode::Ask)
    });
    r.ctl.subscribe(r.peer(), true).unwrap();
    // The host's user approves once.
    r.p.b.answer_prompt(true);
    wait_until("state after approval", WAIT, || r.state().is_some());
    r.ctl
        .command(r.peer(), "chrome", CmdKind::PlayPause, 0)
        .unwrap();
    wait_until("command under the activity", WAIT, || {
        !r.host.cmds.lock().unwrap().is_empty()
    });
    let grants = r.host.grants.lock().unwrap().clone();
    assert!(grants.iter().all(|g| g.activity() != 0));
}

#[test]
fn a_declined_prompt_is_reported() {
    let r = Rig::new();
    r.p.b.set(Capability::Media, Mode::Ask);
    wait_until("controller sees ask", WAIT, || {
        r.p.a.remote_mode(&r.p.b, Capability::Media) == Some(Mode::Ask)
    });
    r.ctl.subscribe(r.peer(), true).unwrap();
    r.p.b.answer_prompt(false);
    wait_until("declined", WAIT, || {
        r.events
            .refused
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == "declined")
    });
    assert!(r.state().is_none());
}

#[test]
fn a_host_without_an_adapter_answers_with_an_empty_state() {
    let p = Pair::new();
    let ctl = p.a.node.service::<Media>().unwrap();
    let peer = p.b.id();
    ctl.subscribe(peer, true).unwrap();
    wait_until("state", WAIT, || ctl.state_of(peer).is_some());
    let st = ctl.state_of(peer).unwrap();
    assert!(st.sessions.is_empty());
    assert_eq!(st.unavailable, "unsupported");
    // Commands with no adapter do nothing and do not break the session.
    ctl.command(peer, "x", CmdKind::Next, 0).unwrap();
    assert!(p.a.connected_to(&p.b));
}

#[test]
fn bad_session_ids_are_refused_before_sending() {
    let r = Rig::new();
    assert!(r.ctl.command(r.peer(), "", CmdKind::Next, 0).is_err());
    let long = "x".repeat(1000);
    assert!(r.ctl.command(r.peer(), &long, CmdKind::Next, 0).is_err());
}
