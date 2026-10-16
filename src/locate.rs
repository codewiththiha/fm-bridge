use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub(crate) const BINARY_NAME: &str = "FMBridge";

/// Environment variable read by `Bridge::from_env`.
pub const BINARY_ENV_VAR: &str = "FM_BRIDGE_BIN";

pub(crate) fn from_env_value(raw: Option<OsString>) -> Result<PathBuf> {
    match raw {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Err(Error::BinaryNotFound(PathBuf::from(format!(
            "${BINARY_ENV_VAR}"
        )))),
    }
}

/// `Contents/MacOS` first, then `Contents/Resources`, as Apple lays out tools.
pub(crate) fn bundled_in(exe_dir: &Path) -> Result<PathBuf> {
    let candidates = [
        exe_dir.join(BINARY_NAME),
        exe_dir.join("..").join("Resources").join(BINARY_NAME),
    ];
    candidates
        .iter()
        .find(|c| c.is_file())
        .map(|c| std::fs::canonicalize(c).unwrap_or_else(|_| c.clone()))
        .ok_or_else(|| Error::BinaryNotFound(exe_dir.join(BINARY_NAME)))
}

pub(crate) fn bundled() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| Error::BinaryNotFound(exe.clone()))?;
    bundled_in(dir)
}

/// Development lookup; trusts the environment, so never ship it.
pub(crate) fn discover() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = from_env_value(std::env::var_os(BINARY_ENV_VAR)) {
        candidates.push(path);
    }
    if let Ok(cwd) = std::env::current_dir() {
        for profile in ["release", "debug"] {
            candidates.push(cwd.join("swift/.build").join(profile).join(BINARY_NAME));
        }
        candidates.push(cwd.join(BINARY_NAME));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_owned))
    {
        candidates.push(dir.join(BINARY_NAME));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(BINARY_NAME)));
    }
    candidates
        .into_iter()
        .find(|c| c.is_file())
        .ok_or_else(|| Error::BinaryNotFound(PathBuf::from(BINARY_NAME)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_value_must_be_set_and_non_empty() {
        for raw in [None, Some(OsString::new())] {
            assert!(matches!(from_env_value(raw), Err(Error::BinaryNotFound(_))));
        }
        let path = from_env_value(Some("/x/FMBridge".into())).unwrap();
        assert_eq!(path, PathBuf::from("/x/FMBridge"));
    }

    #[test]
    fn bundled_prefers_macos_then_resources() {
        let app = tempfile::tempdir().unwrap();
        let macos = app.path().join("Contents/MacOS");
        let resources = app.path().join("Contents/Resources");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::create_dir_all(&resources).unwrap();
        assert!(bundled_in(&macos).is_err());

        std::fs::write(resources.join(BINARY_NAME), "").unwrap();
        assert!(bundled_in(&macos).unwrap().ends_with("Resources/FMBridge"));

        std::fs::write(macos.join(BINARY_NAME), "").unwrap();
        assert!(bundled_in(&macos).unwrap().ends_with("MacOS/FMBridge"));
    }
}
