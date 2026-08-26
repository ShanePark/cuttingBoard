use crate::{
    models::UiSettings,
    storage::{load_settings, save_settings},
};
use std::path::Path;
use tauri::{PhysicalSize, Runtime, Window};

pub(crate) fn persist_window_geometry<R: Runtime>(window: &Window<R>, settings_path: &Path) {
    let (Ok(size), Ok(position), Ok(scale_factor)) = (
        window.inner_size(),
        window.outer_position(),
        window.scale_factor(),
    ) else {
        return;
    };
    let logical_size = size.to_logical::<u32>(scale_factor);
    let settings = load_settings(settings_path).unwrap_or_default();
    let updated = UiSettings {
        window_width: logical_size.width.max(560),
        window_height: logical_size.height.max(420),
        window_x: Some(position.x),
        window_y: Some(position.y),
        window_geometry_logical: true,
        ..settings
    };
    if let Err(error) = save_settings(settings_path, updated) {
        eprintln!("Could not persist window geometry: {error}");
    }
}

pub(crate) fn migrate_legacy_window_size(
    mut settings: UiSettings,
    scale_factor: f64,
) -> Option<UiSettings> {
    if settings.window_geometry_logical
        || settings.window_x.is_none()
        || settings.window_y.is_none()
        || !scale_factor.is_finite()
        || scale_factor <= 1.0
    {
        return None;
    }

    let logical_size = PhysicalSize::new(settings.window_width, settings.window_height)
        .to_logical::<u32>(scale_factor);
    settings.window_width = logical_size.width.max(560);
    settings.window_height = logical_size.height.max(420);
    settings.window_geometry_logical = true;
    Some(settings)
}

pub(crate) fn migrate_startup_window_settings<R: Runtime>(
    window: &Window<R>,
    settings_path: &Path,
    settings: UiSettings,
) -> UiSettings {
    let Ok(scale_factor) = window.scale_factor() else {
        return settings;
    };
    let Some(migrated) = migrate_legacy_window_size(settings.clone(), scale_factor) else {
        return settings;
    };
    if let Err(error) = save_settings(settings_path, migrated.clone()) {
        eprintln!("Could not persist migrated window geometry: {error}");
    }
    migrated
}

#[cfg(test)]
mod tests {
    use super::migrate_legacy_window_size;
    use crate::models::UiSettings;
    use tauri::PhysicalSize;

    #[test]
    fn persists_inner_size_in_logical_pixels() {
        let logical_size = PhysicalSize::new(2_160, 1_440).to_logical::<u32>(2.0);

        assert_eq!(logical_size.width, 1_080);
        assert_eq!(logical_size.height, 720);
    }

    #[test]
    fn migrates_unmarked_legacy_physical_window_size() {
        let settings = UiSettings {
            window_width: 2_160,
            window_height: 1_440,
            window_x: Some(40),
            window_y: Some(78),
            ..UiSettings::default()
        };
        let migrated =
            migrate_legacy_window_size(settings, 2.0).expect("legacy physical size should migrate");

        assert_eq!(migrated.window_width, 1_080);
        assert_eq!(migrated.window_height, 720);
    }

    #[test]
    fn does_not_migrate_a_valid_logical_size() {
        let settings = UiSettings {
            window_width: 1_200,
            window_height: 800,
            window_x: Some(40),
            window_y: Some(78),
            window_geometry_logical: true,
            ..UiSettings::default()
        };

        assert!(migrate_legacy_window_size(settings, 2.0).is_none());
    }
}
