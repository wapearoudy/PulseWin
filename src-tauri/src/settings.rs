//! The handful of choices the panel keeps between launches.
//!
//! The original stores these in `UserDefaults`; on Windows the equivalent is a
//! small JSON file under `%APPDATA%`. Only the things the *panel* owns live
//! here — where it is docked, and later its size — never a reading: a snapshot
//! is fetched fresh every time, and a remembered number would be a stale one
//! drawn as if it were current.

use std::path::PathBuf;

use crate::PanelEdge;

/// Everything the panel remembers. Every field is optional, so a file written
/// by an older build still loads and one written by a newer build is not lost
/// when an older build reads it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PanelSettings {
    /// Which edge the rail was last docked to.
    pub dock_edge: Option<PanelEdge>,
    pub rail_position: Option<(i32, i32)>,
    pub facing_edge: Option<PanelEdge>,
    /// Preferred display identity. If it is unplugged we keep its record and
    /// temporarily use the primary display until it returns.
    pub display: Option<String>,
    pub per_display: std::collections::BTreeMap<String, DisplayPlacement>,
    /// The physical coordinate space of rail_position, including the actual
    /// fallback display when the preferred display is disconnected. Ratios
    /// are for display changes; a measured rail growing on this same space
    /// must keep the absolute point the user chose.
    pub rail_space: Option<crate::desktop::Display>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayPlacement {
    pub dock_edge: Option<PanelEdge>,
    pub facing_edge: Option<PanelEdge>,
    pub horizontal_ratio: f64,
    pub vertical_ratio: f64,
}

impl Default for DisplayPlacement {
    fn default() -> Self {
        Self { dock_edge: Some(PanelEdge::Right), facing_edge: Some(PanelEdge::Right), horizontal_ratio: 1.0, vertical_ratio: 0.5 }
    }
}

/// `%APPDATA%\PulseWin\panel.json`, or nothing when there is no APPDATA.
pub fn file() -> Option<PathBuf> {
    directory().map(|dir| dir.join("panel.json"))
}

/// `%APPDATA%\PulseWin` — where the panel's settings and every pasted
/// credential live.
pub fn directory() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|base| base.join("PulseWin"))
}

/// The canonical fields edited by Settings. The on-disk document is encrypted
/// with current-user Windows DPAPI and retains all vendor-specific fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Credential {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Where a pasted credential for `provider` is stored.
pub fn credential_file(provider: &str) -> Option<PathBuf> {
    // The id reaches the filesystem, so it has to be a name and nothing else:
    // `../` in an id would write anywhere the process can reach.
    if !crate::credential_store::valid_provider_id(provider) {
        return None;
    }
    directory().map(|dir| dir.join(format!("{provider}.json")))
}

/// The stored credential, or the empty one.
pub fn read_credential(provider: &str) -> Credential {
    read_credential_checked(provider).unwrap_or_default()
}

/// Callers that show credentials in Settings or refresh a provider must use
/// this checked route so migration/decryption errors remain visible.
pub fn read_credential_checked(provider: &str) -> std::io::Result<Credential> {
    let Some(document) = read_credential_document(provider)? else { return Ok(Credential::default()); };
    Ok(Credential {
        api_key: crate::credentials::dig_first_str(&document, &["apiKey", "api_key", "key", "token", "cookie"]),
        base_url: crate::credentials::dig_first_str(&document, &["baseUrl", "base_url", "serverAddress", "address"]),
    })
}

pub fn read_credential_document(provider: &str) -> std::io::Result<Option<serde_json::Value>> {
    let path = credential_file(provider).ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::InvalidInput, "Invalid provider id or APPDATA is unavailable"))?;
    crate::credential_store::read_at(&path, provider)
}

/// Store a credential, trimming the blanks off it.
///
/// An empty field is **removed rather than written as `""`**: a provider that
/// reads an empty string as a key reports "refused" instead of "not
/// configured", which is the wrong one of the two answers.
pub fn write_credential(provider: &str, credential: &Credential) -> std::io::Result<()> {
    let Some(path) = credential_file(provider) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("`{provider}` is not a usable provider id"),
        ));
    };
    crate::credential_store::update_at(&path, provider, credential.api_key.as_deref(), credential.base_url.as_deref())
}

/// Read the settings, or the defaults.
///
/// A settings file that cannot be read is **not** an error worth reporting: the
/// panel has to open either way, and the worst case is that it opens on the
/// edge it ships docked to. A corrupt file is treated the same as a missing
/// one rather than taking the app down before it has drawn anything.
pub fn load() -> PanelSettings {
    file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Write the settings, ignoring a failure.
///
/// The same reasoning as `load`: a panel that cannot save where it was put is
/// still a panel that was put there.
pub fn save(settings: &PanelSettings) {
    let Some(path) = file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, to_json(settings));
}

/// Parse, falling back to the defaults on anything unexpected.
pub fn parse(text: &str) -> PanelSettings {
    serde_json::from_str(text).unwrap_or_default()
}

/// Serialize anything this module stores. Pretty-printed on purpose: these
/// files are small, and a reader who opens one should be able to read it.
pub fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_docked_edge() {
        let settings = PanelSettings {
            dock_edge: Some(PanelEdge::Top),
            ..Default::default()
        };
        let text = to_json(&settings);
        assert!(text.contains("top"), "wrote `{text}`");
        assert_eq!(parse(&text), settings);
    }

    #[test]
    fn floating_rail_position_and_facing_survive_a_round_trip() {
        let saved = PanelSettings { dock_edge: None, rail_position: Some((-800, 120)), facing_edge: Some(PanelEdge::Left), ..Default::default() };
        assert_eq!(parse(&to_json(&saved)), saved);
        assert_eq!(parse(r#"{"dockEdge":"right"}"#).rail_position, None);
    }

    #[test]
    fn every_edge_survives_a_round_trip() {
        for edge in [PanelEdge::Left, PanelEdge::Right, PanelEdge::Top] {
            let settings = PanelSettings {
                dock_edge: Some(edge),
                ..Default::default()
            };
            assert_eq!(parse(&to_json(&settings)).dock_edge, Some(edge));
        }
    }

    #[test]
    fn per_display_positions_round_trip_without_losing_unplugged_displays() {
        let mut saved = PanelSettings::default();
        saved.display = Some("monitor-b".into());
        saved.per_display.insert("monitor-a".into(), DisplayPlacement { horizontal_ratio: 0.25, vertical_ratio: 0.8, ..Default::default() });
        saved.per_display.insert("monitor-b".into(), DisplayPlacement { dock_edge: None, facing_edge: Some(PanelEdge::Left), horizontal_ratio: 0.2, vertical_ratio: 0.1 });
        assert_eq!(parse(&to_json(&saved)), saved);
        let old = parse(r#"{"dockEdge":"left","railPosition":[-1800,300],"facingEdge":"left"}"#);
        assert_eq!(old.rail_position, Some((-1800, 300)));
        assert!(old.per_display.is_empty());
        assert!(old.display.is_none());
    }

    #[test]
    fn absolute_rail_coordinate_space_survives_restart_and_legacy_files_default_to_none() {
        let mut saved=PanelSettings{rail_position:Some((720,80)),display:Some("physical".into()),..Default::default()};
        saved.rail_space=Some(crate::desktop::Display{id:"physical".into(),bounds:crate::desktop::Rect{left:0,top:0,right:3840,bottom:2160},work_area:crate::desktop::Rect{left:0,top:0,right:3840,bottom:2082},scale:1.5});
        assert_eq!(parse(&to_json(&saved)),saved);
        assert!(parse(r#"{"railPosition":[720,80]}"#).rail_space.is_none());
    }

    #[test]
    fn a_missing_or_unreadable_file_gives_the_defaults() {
        // The panel has to open either way.
        assert_eq!(parse(""), PanelSettings::default());
        assert_eq!(parse("not json at all"), PanelSettings::default());
        assert_eq!(parse("{}"), PanelSettings::default());
        assert_eq!(parse(r#"{"dockEdge":null}"#), PanelSettings::default());
    }

    #[test]
    fn an_unknown_edge_gives_the_defaults_rather_than_drawing_nothing() {
        // A stored choice that no longer exists falls back, like `BotMarkBody`.
        assert_eq!(parse(r#"{"dockEdge":"bottom"}"#), PanelSettings::default());
    }

    #[test]
    fn an_unknown_field_does_not_lose_the_known_one() {
        assert_eq!(
            parse(r#"{"dockEdge":"left","somethingNew":42}"#).dock_edge,
            Some(PanelEdge::Left),
        );
    }

    #[test]
    fn the_file_sits_under_the_app_data_directory() {
        // Not asserting the exact path — only that it is inside `PulseWin` and
        // called `panel.json`, and that a machine without APPDATA is not a panic.
        if let Some(path) = file() {
            assert!(path.ends_with("panel.json"), "{path:?}");
            assert_eq!(
                path.parent().and_then(|p| p.file_name()),
                Some(std::ffi::OsStr::new("PulseWin")),
            );
        }
    }

    // ---- pasted credentials ------------------------------------------------

    #[test]
    fn a_credential_round_trips_in_the_shape_the_providers_read() {
        let credential = Credential {
            api_key: Some("sk-test".into()),
            base_url: Some("https://box.example".into()),
        };
        let text = to_json(&credential);
        // The providers look for exactly these two names.
        assert!(text.contains("\"apiKey\""), "wrote `{text}`");
        assert!(text.contains("\"baseUrl\""), "wrote `{text}`");
        assert_eq!(
            serde_json::from_str::<Credential>(&text).unwrap(),
            credential,
        );
    }

    #[test]
    fn a_provider_id_cannot_escape_the_settings_directory() {
        // The id reaches the filesystem: `../` would write anywhere.
        assert!(credential_file("..\\..\\evil").is_none());
        assert!(credential_file("../evil").is_none());
        assert!(credential_file("a/b").is_none());
        assert!(credential_file("a b").is_none());
        assert!(credential_file("").is_none());
        assert!(credential_file("deepseek").is_some());
        assert!(credential_file("hugging-face").is_some());
        assert!(credential_file("llm_proxy").is_some());
    }

    #[test]
    fn a_missing_credential_file_reads_as_empty_rather_than_failing() {
        let empty = read_credential("pulsewin-no-such-provider");
        assert_eq!(empty, Credential::default());
    }
}
