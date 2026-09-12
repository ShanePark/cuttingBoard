use crate::{
    models::UiSettings,
    storage::{load_settings, save_settings},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, Weak},
    thread,
    time::{Duration, Instant},
};
use tauri::{PhysicalSize, Runtime, Window};

const GEOMETRY_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowGeometry {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

struct GeometryWorkerState {
    pending: Option<WindowGeometry>,
    in_flight: Option<WindowGeometry>,
    last_persisted: Option<WindowGeometry>,
    flush_waiters: usize,
    shutting_down: bool,
    last_changed: Instant,
}

impl GeometryWorkerState {
    fn new() -> Self {
        Self {
            pending: None,
            in_flight: None,
            last_persisted: None,
            flush_waiters: 0,
            shutting_down: false,
            last_changed: Instant::now(),
        }
    }
}

struct WindowGeometryPersistenceInner {
    settings_path: PathBuf,
    settings_io: Arc<Mutex<()>>,
    state: Mutex<GeometryWorkerState>,
    wake: Condvar,
    #[cfg(test)]
    writes: std::sync::atomic::AtomicUsize,
}

pub(crate) struct WindowGeometryPersistence {
    inner: Arc<WindowGeometryPersistenceInner>,
}

impl WindowGeometryPersistence {
    pub(crate) fn new(settings_path: PathBuf, settings_io: Arc<Mutex<()>>) -> Self {
        let inner = Arc::new(WindowGeometryPersistenceInner {
            settings_path,
            settings_io,
            state: Mutex::new(GeometryWorkerState::new()),
            wake: Condvar::new(),
            #[cfg(test)]
            writes: std::sync::atomic::AtomicUsize::new(0),
        });
        let worker = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("window-geometry-persistence".into())
            .spawn(move || run_worker(worker))
            .expect("window geometry persistence worker could not start");
        Self { inner }
    }

    pub(crate) fn record_window<R: Runtime>(&self, window: &Window<R>) {
        let Some(geometry) = capture_window_geometry(window) else {
            return;
        };
        self.record_geometry(geometry);
    }

    fn record_geometry(&self, geometry: WindowGeometry) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        if state.shutting_down
            || state.pending == Some(geometry)
            || (state.pending.is_none() && state.in_flight == Some(geometry))
            || (state.pending.is_none()
                && state.in_flight.is_none()
                && state.last_persisted == Some(geometry))
        {
            return;
        }
        state.pending = Some(geometry);
        state.last_changed = Instant::now();
        self.inner.wake.notify_all();
    }

    pub(crate) fn flush_window<R: Runtime>(&self, window: &Window<R>) {
        self.record_window(window);
        self.flush();
    }

    fn flush(&self) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.flush_waiters = state.flush_waiters.saturating_add(1);
        self.inner.wake.notify_all();
        while state.pending.is_some() || state.in_flight.is_some() {
            state = match self.inner.wake.wait(state) {
                Ok(state) => state,
                Err(_) => return,
            };
        }
        state.flush_waiters = state.flush_waiters.saturating_sub(1);
        self.inner.wake.notify_all();
    }

    #[cfg(test)]
    fn write_count(&self) -> usize {
        self.inner.writes.load(std::sync::atomic::Ordering::Acquire)
    }

    #[cfg(test)]
    fn in_flight_geometry(&self) -> Option<WindowGeometry> {
        self.inner.state.lock().ok()?.in_flight
    }
}

impl Drop for WindowGeometryPersistence {
    fn drop(&mut self) {
        let has_pending_write = self
            .inner
            .state
            .lock()
            .map(|state| state.pending.is_some() || state.in_flight.is_some())
            .unwrap_or(false);
        if has_pending_write {
            self.flush();
        }
        if let Ok(mut state) = self.inner.state.lock() {
            state.shutting_down = true;
            self.inner.wake.notify_all();
        }
    }
}

fn run_worker(worker: Weak<WindowGeometryPersistenceInner>) {
    loop {
        let Some(inner) = worker.upgrade() else {
            return;
        };
        let pending = {
            let Ok(mut state) = inner.state.lock() else {
                return;
            };
            loop {
                if state.shutting_down {
                    return;
                }
                let Some(current) = state.pending else {
                    state = match inner.wake.wait(state) {
                        Ok(state) => state,
                        Err(_) => return,
                    };
                    continue;
                };
                if state.flush_waiters > 0 {
                    state.pending = None;
                    state.in_flight = Some(current);
                    break current;
                }
                let deadline = state.last_changed + GEOMETRY_DEBOUNCE;
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    state.pending = None;
                    state.in_flight = Some(current);
                    break current;
                }
                state = match inner.wake.wait_timeout(state, remaining) {
                    Ok((state, _)) => state,
                    Err(_) => return,
                };
            }
        };

        let persisted = persist_geometry(&inner.settings_path, pending, &inner.settings_io);
        let write_succeeded = persisted.is_ok();
        if let Err(error) = persisted {
            eprintln!("Could not persist window geometry: {error}");
        }
        let Ok(mut state) = inner.state.lock() else {
            return;
        };
        state.in_flight = None;
        if write_succeeded {
            state.last_persisted = Some(pending);
            if state.pending == Some(pending) {
                state.pending = None;
            }
            #[cfg(test)]
            inner
                .writes
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
        inner.wake.notify_all();
    }
}

fn capture_window_geometry<R: Runtime>(window: &Window<R>) -> Option<WindowGeometry> {
    let (Ok(size), Ok(position), Ok(scale_factor)) = (
        window.inner_size(),
        window.outer_position(),
        window.scale_factor(),
    ) else {
        return None;
    };
    let logical_size = size.to_logical::<u32>(scale_factor);
    Some(WindowGeometry {
        width: logical_size.width.max(560),
        height: logical_size.height.max(420),
        x: position.x,
        y: position.y,
    })
}

fn persist_geometry(
    settings_path: &Path,
    geometry: WindowGeometry,
    settings_io: &Mutex<()>,
) -> Result<(), String> {
    let _settings_io = settings_io
        .lock()
        .map_err(|_| "Settings storage lock was poisoned.".to_string())?;
    let settings = load_settings(settings_path).unwrap_or_default();
    let updated = UiSettings {
        window_width: geometry.width,
        window_height: geometry.height,
        window_x: Some(geometry.x),
        window_y: Some(geometry.y),
        window_geometry_logical: true,
        ..settings
    };
    save_settings(settings_path, updated).map(|_| ())
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
    use super::{
        migrate_legacy_window_size, persist_geometry, WindowGeometry, WindowGeometryPersistence,
    };
    use crate::{
        models::UiSettings,
        storage::{load_settings, save_settings},
    };
    use std::{
        fs,
        path::Path,
        sync::{mpsc, Arc, Mutex},
        thread,
        time::{Duration, Instant},
    };
    use tauri::PhysicalSize;

    fn geometry(width: u32, height: u32, x: i32, y: i32) -> WindowGeometry {
        WindowGeometry {
            width,
            height,
            x,
            y,
        }
    }

    fn persistence(path: &Path) -> WindowGeometryPersistence {
        WindowGeometryPersistence::new(path.to_path_buf(), Arc::new(Mutex::new(())))
    }

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

    #[test]
    fn coalesces_geometry_burst_into_one_write_and_keeps_latest_geometry() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        save_settings(
            &path,
            UiSettings {
                theme_mode: "light".into(),
                scan_interval_ms: 1_000,
                ..UiSettings::default()
            },
        )
        .unwrap();
        let persistence = persistence(&path);
        for index in 0..100 {
            persistence.record_geometry(geometry(
                800 + index,
                600 + index,
                index as i32,
                (index * 2) as i32,
            ));
        }

        persistence.flush();

        let settings = load_settings(&path).unwrap();
        assert_eq!(settings.window_width, 899);
        assert_eq!(settings.window_height, 699);
        assert_eq!(settings.window_x, Some(99));
        assert_eq!(settings.window_y, Some(198));
        assert_eq!(settings.theme_mode, "light");
        assert_eq!(settings.scan_interval_ms, 1_000);
        assert_eq!(persistence.write_count(), 1);

        persistence.flush();
        assert_eq!(persistence.write_count(), 1);
    }

    #[test]
    fn keeps_latest_geometry_when_it_repeats_during_an_in_flight_write() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        save_settings(&path, UiSettings::default()).unwrap();
        let settings_io = Arc::new(Mutex::new(()));
        let persistence = WindowGeometryPersistence::new(path.clone(), Arc::clone(&settings_io));
        let settings_guard = settings_io.lock().unwrap();
        let first = geometry(1_280, 800, 64, 96);
        let intermediate = geometry(1_300, 820, 80, 112);
        persistence.record_geometry(first);
        let started = Instant::now();
        while persistence.in_flight_geometry() != Some(first) {
            assert!(started.elapsed() < Duration::from_secs(2));
            thread::sleep(Duration::from_millis(5));
        }
        persistence.record_geometry(intermediate);
        persistence.record_geometry(first);
        drop(settings_guard);
        persistence.flush();

        let settings = load_settings(&path).unwrap();
        assert_eq!(settings.window_width, first.width);
        assert_eq!(settings.window_height, first.height);
        assert_eq!(settings.window_x, Some(first.x));
        assert_eq!(settings.window_y, Some(first.y));
        assert_eq!(persistence.write_count(), 1);
    }

    #[test]
    fn serializes_geometry_and_command_settings_writes_without_clobbering_either() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        save_settings(&path, UiSettings::default()).unwrap();
        let settings_io = Arc::new(Mutex::new(()));
        let persistence = WindowGeometryPersistence::new(path.clone(), Arc::clone(&settings_io));
        let (command_ready_tx, command_ready_rx) = mpsc::channel();
        let (release_command_tx, release_command_rx) = mpsc::channel();
        let command_path = path.clone();
        let command_io = Arc::clone(&settings_io);
        let command = thread::spawn(move || {
            let _settings_io = command_io.lock().unwrap();
            let mut settings = load_settings(&command_path).unwrap();
            settings.theme_mode = "light".into();
            settings.scan_interval_ms = 5_000;
            save_settings(&command_path, settings).unwrap();
            command_ready_tx.send(()).unwrap();
            release_command_rx.recv().unwrap();
        });
        command_ready_rx.recv().unwrap();
        persistence.record_geometry(geometry(1_280, 800, 64, 96));
        release_command_tx.send(()).unwrap();
        persistence.flush();
        command.join().unwrap();

        let settings = load_settings(&path).unwrap();
        assert_eq!(settings.theme_mode, "light");
        assert_eq!(settings.scan_interval_ms, 5_000);
        assert_eq!(settings.window_width, 1_280);
        assert_eq!(settings.window_height, 800);
        assert_eq!(settings.window_x, Some(64));
        assert_eq!(settings.window_y, Some(96));
    }

    #[test]
    fn flush_completes_when_geometry_write_fails() {
        let temporary = tempfile::tempdir().unwrap();
        let parent_file = temporary.path().join("not-a-directory");
        fs::write(&parent_file, "settings").unwrap();
        let path = parent_file.join("settings.json");
        let persistence = persistence(&path);
        persistence.record_geometry(geometry(1_280, 800, 64, 96));

        persistence.flush();

        assert_eq!(persistence.write_count(), 0);
    }

    #[test]
    fn flush_persists_latest_geometry_before_persistence_shutdown() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        save_settings(&path, UiSettings::default()).unwrap();
        let persistence = persistence(&path);
        persistence.record_geometry(geometry(1_400, 900, 12, 34));

        drop(persistence);

        let settings = load_settings(&path).unwrap();
        assert_eq!(settings.window_width, 1_400);
        assert_eq!(settings.window_height, 900);
        assert_eq!(settings.window_x, Some(12));
        assert_eq!(settings.window_y, Some(34));
    }

    #[test]
    #[ignore = "manual performance benchmark"]
    fn benchmark_geometry_storage_burst() {
        let temporary = tempfile::tempdir().unwrap();
        let baseline_path = temporary.path().join("baseline-settings.json");
        let coalesced_path = temporary.path().join("coalesced-settings.json");
        save_settings(&baseline_path, UiSettings::default()).unwrap();
        save_settings(&coalesced_path, UiSettings::default()).unwrap();
        let settings_io = Arc::new(Mutex::new(()));
        let events = 100;
        let started = Instant::now();
        for index in 0..events {
            persist_geometry(
                &baseline_path,
                geometry(800 + index, 600 + index, index as i32, (index * 2) as i32),
                &settings_io,
            )
            .unwrap();
        }
        let baseline_elapsed = started.elapsed();

        let persistence = WindowGeometryPersistence::new(coalesced_path, settings_io);
        let started = Instant::now();
        for index in 0..events {
            persistence.record_geometry(geometry(
                800 + index,
                600 + index,
                index as i32,
                (index * 2) as i32,
            ));
        }
        let enqueue_elapsed = started.elapsed();
        let flush_started = Instant::now();
        persistence.flush();
        let flush_elapsed = flush_started.elapsed();
        println!(
            "geometry_benchmark events={events} baseline_ms={:.3} baseline_writes={events} enqueue_ms={:.3} coalesced_flush_ms={:.3} coalesced_writes={}",
            baseline_elapsed.as_secs_f64() * 1000.0,
            enqueue_elapsed.as_secs_f64() * 1000.0,
            flush_elapsed.as_secs_f64() * 1000.0,
            persistence.write_count(),
        );
    }
}
