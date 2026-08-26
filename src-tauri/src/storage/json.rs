use crate::models::UiSettings;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(super) fn read_json_or_default<T>(path: &Path) -> Result<T, String>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(&text)
        .map_err(|error| format!("Could not parse {}: {error}", path.display()))
}

pub(super) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let temporary = temporary_path(path);
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Could not serialize {}: {error}", path.display()))?;
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("Could not create {}: {error}", temporary.display()))?;
        file.write_all(&bytes)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Could not write {}: {error}", temporary.display()))?;
    }
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not replace {}: {error}", path.display()))?;
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

pub fn load_settings(path: &Path) -> Result<UiSettings, String> {
    read_json_or_default::<UiSettings>(path).map(UiSettings::normalized)
}

pub fn save_settings(path: &Path, settings: UiSettings) -> Result<UiSettings, String> {
    let normalized = settings.normalized();
    write_json_atomic(path, &normalized)?;
    Ok(normalized)
}
