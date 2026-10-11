//! Pulse 1.9 CrushReader: discover declared stores; dollars are not token counts.
use super::*;
pub(super) fn roots(home: &Path, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut registries = vec![];
    if let Some(root) = env("CRUSH_GLOBAL_DATA").filter(|s| !s.trim().is_empty()) {
        registries.push(PathBuf::from(root).join("projects.json"))
    }
    let data = env("XDG_DATA_HOME")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    registries.push(data.join("crush/projects.json"));
    if let Some(local) = env("LOCALAPPDATA") {
        registries.push(PathBuf::from(local).join("crush/projects.json"))
    }
    registries.push(home.join("AppData/Local/crush/projects.json"));
    let mut roots = vec![];
    for registry in registries {
        if fs::metadata(&registry).is_ok_and(|m| m.len() <= 8 * 1024 * 1024) {
            if let Ok(raw) = fs::read(&registry) {
                if let Ok(value) = serde_json::from_slice::<Value>(&raw) {
                    roots.extend(databases(&value));
                }
            }
        }
        roots.push(registry);
    }
    roots.sort();
    roots.dedup();
    roots
}
fn databases(registry: &Value) -> Vec<PathBuf> {
    registry["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_FILES)
        .filter_map(|p| {
            let path = super::measured_readers::text(&p["path"])?;
            let directory = PathBuf::from(super::measured_readers::text(&p["data_dir"])?);
            Some(
                if directory.is_absolute() {
                    directory
                } else {
                    PathBuf::from(path).join(directory)
                }
                .join("crush.db"),
            )
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn registry_is_only_discovery_even_with_dollar_or_token_like_fields() {
        let r = databases(
            &json!({"projects":[{"path":"C:/project","data_dir":".crush","cost":100,"input_tokens":999},{"path":"C:/other","data_dir":"D:/crush-data"},{"path":"C:/bad"}]}),
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], PathBuf::from("C:/project/.crush/crush.db"));
        assert_eq!(r[1], PathBuf::from("D:/crush-data/crush.db"));
    }
}
