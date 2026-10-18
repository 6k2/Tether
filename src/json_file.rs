//! Loading JSON state files without losing them: a file that fails to parse is moved
//! aside to `<name>.bak` before defaults are used, so the next save can't overwrite the
//! user's data.

use std::path::Path;

use serde::de::DeserializeOwned;

/// Reads `path`, or returns defaults when it is missing or corrupt (backing it up).
pub fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(text) = std::fs::read_to_string(path) else { return T::default() };
    match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => {
            let _ = std::fs::rename(path, path.with_extension("json.bak"));
            T::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_files_are_backed_up_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "{not json").unwrap();
        let loaded: Vec<u32> = load_or_default(&path);
        assert!(loaded.is_empty());
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("state.json.bak")).unwrap(), "{not json");
    }

    #[test]
    fn missing_and_valid_files_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        assert!(load_or_default::<Vec<u32>>(&path).is_empty());
        std::fs::write(&path, "[1,2]").unwrap();
        assert_eq!(load_or_default::<Vec<u32>>(&path), vec![1, 2]);
    }
}
