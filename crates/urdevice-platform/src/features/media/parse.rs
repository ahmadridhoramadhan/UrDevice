//! Small parsers shared by the desktop media adapters. They take text that
//! other programs produce (`wpctl`, `pactl`, `osascript`, MPRIS metadata), so
//! they never panic and never trust a length.

// Each OS uses a different subset.
#![allow(dead_code)]

/// Output volume and mute as read from the system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Volume {
    /// 0.0 to 1.0 (clamped).
    pub level: f32,
    pub muted: bool,
}

/// `wpctl get-volume @DEFAULT_AUDIO_SINK@`: `Volume: 0.40` or
/// `Volume: 0.40 [MUTED]`.
pub(crate) fn wpctl_volume(out: &str) -> Option<Volume> {
    let rest = out.trim().strip_prefix("Volume:")?;
    let level: f32 = rest.split_whitespace().next()?.parse().ok()?;
    if !level.is_finite() || level < 0.0 {
        return None;
    }
    Some(Volume {
        level: level.min(1.0),
        muted: rest.contains("[MUTED]"),
    })
}

/// `pactl get-sink-volume @DEFAULT_SINK@`: the first `NN%` is the level, for
/// example `Volume: front-left: 26214 /  40% / -23.94 dB, ...`.
pub(crate) fn pactl_volume(out: &str) -> Option<f32> {
    let pct = out
        .split_whitespace()
        .find(|w| w.ends_with('%') && w.len() > 1)?
        .trim_end_matches('%');
    let v: f32 = pct.parse().ok()?;
    (v.is_finite() && v >= 0.0).then(|| (v / 100.0).min(1.0))
}

/// `pactl get-sink-mute @DEFAULT_SINK@`: `Mute: yes` or `Mute: no`.
pub(crate) fn pactl_muted(out: &str) -> Option<bool> {
    match out.trim().strip_prefix("Mute:")?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// `osascript` prints `50` for the output volume. `missing value` (no
/// software volume) gives `None`.
pub(crate) fn osascript_volume(out: &str) -> Option<f32> {
    let v: f32 = out.trim().parse().ok()?;
    (v.is_finite() && (0.0..=100.0).contains(&v)).then(|| v / 100.0)
}

/// `osascript` prints `true` or `false` for the muted flag.
pub(crate) fn osascript_bool(out: &str) -> Option<bool> {
    match out.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// The local path of a `file://` URL (MPRIS `mpris:artUrl`), with
/// percent-escapes decoded. Other schemes, remote hosts, and paths that are
/// not absolute give `None`: the host never fetches a URL.
pub(crate) fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let h = bytes.get(i + 1..i + 3)?;
            let h = std::str::from_utf8(h).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    (!s.contains('\0')).then_some(s)
}

/// An MPRIS bus name is `org.mpris.MediaPlayer2.<name>`; the controller sends
/// it back as a session id, so check its shape before using it.
pub(crate) fn is_mpris_bus_name(s: &str) -> bool {
    const PREFIX: &str = "org.mpris.MediaPlayer2.";
    s.len() > PREFIX.len()
        && s.len() <= 255
        && s.starts_with(PREFIX)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A readable app name from a Windows AppUserModelId: `Spotify.exe` becomes
/// `Spotify`, `SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify` becomes
/// `Spotify`, `chrome` becomes `Chrome`.
pub(crate) fn app_name_from_aumid(aumid: &str) -> String {
    let base = aumid.rsplit('!').next().unwrap_or(aumid);
    let base = base.rsplit(['\\', '/']).next().unwrap_or(base);
    let base = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    let mut chars = base.chars();
    match chars.next() {
        Some(c) if c.is_lowercase() => c.to_uppercase().chain(chars).collect(),
        Some(c) => std::iter::once(c).chain(chars).collect(),
        None => aumid.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpctl() {
        assert_eq!(
            wpctl_volume("Volume: 0.40\n"),
            Some(Volume {
                level: 0.4,
                muted: false
            })
        );
        assert_eq!(
            wpctl_volume("Volume: 1.20 [MUTED]"),
            Some(Volume {
                level: 1.0,
                muted: true
            })
        );
        assert_eq!(wpctl_volume("Volume: abc"), None);
        assert_eq!(wpctl_volume("Volume: -1"), None);
        assert_eq!(wpctl_volume(""), None);
    }

    #[test]
    fn pactl() {
        let out = "Volume: front-left: 26214 /  40% / -23.94 dB,   front-right: 26214 /  40% / -23.94 dB\n";
        assert_eq!(pactl_volume(out), Some(0.4));
        assert_eq!(pactl_volume("Volume: nonsense"), None);
        assert_eq!(pactl_muted("Mute: yes\n"), Some(true));
        assert_eq!(pactl_muted("Mute: no"), Some(false));
        assert_eq!(pactl_muted("Mute: maybe"), None);
    }

    #[test]
    fn osascript() {
        assert_eq!(osascript_volume("50\n"), Some(0.5));
        assert_eq!(osascript_volume("missing value"), None);
        assert_eq!(osascript_volume("500"), None);
        assert_eq!(osascript_bool("true\n"), Some(true));
        assert_eq!(osascript_bool("x"), None);
    }

    #[test]
    fn file_urls() {
        assert_eq!(
            file_url_path("file:///tmp/a%20b.png").as_deref(),
            Some("/tmp/a b.png")
        );
        assert_eq!(
            file_url_path("file://localhost/x.png").as_deref(),
            Some("/x.png")
        );
        assert_eq!(file_url_path("https://example.com/a.png"), None);
        assert_eq!(file_url_path("file://server/share/a.png"), None);
        assert_eq!(file_url_path("file:///a%zz"), None);
        assert_eq!(file_url_path("file:///a%2"), None);
        assert_eq!(file_url_path("file:///a%00b"), None);
        assert_eq!(file_url_path("data:image/png;base64,AAAA"), None);
    }

    #[test]
    fn bus_names() {
        assert!(is_mpris_bus_name("org.mpris.MediaPlayer2.spotify"));
        assert!(is_mpris_bus_name(
            "org.mpris.MediaPlayer2.chromium.instance123"
        ));
        assert!(!is_mpris_bus_name("org.mpris.MediaPlayer2."));
        assert!(!is_mpris_bus_name("org.freedesktop.Notifications"));
        assert!(!is_mpris_bus_name("org.mpris.MediaPlayer2.a b"));
        assert!(!is_mpris_bus_name(&format!(
            "org.mpris.MediaPlayer2.{}",
            "a".repeat(300)
        )));
    }

    #[test]
    fn aumid_names() {
        assert_eq!(app_name_from_aumid("Spotify.exe"), "Spotify");
        assert_eq!(
            app_name_from_aumid("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "Spotify"
        );
        assert_eq!(app_name_from_aumid("chrome"), "Chrome");
        assert_eq!(app_name_from_aumid("MSEdge"), "MSEdge");
        assert_eq!(app_name_from_aumid(""), "");
    }
}
