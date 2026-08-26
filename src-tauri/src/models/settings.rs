use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettings {
    pub theme_mode: String,
    pub scan_interval_ms: u64,
    pub window_width: u32,
    pub window_height: u32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    #[serde(default)]
    pub window_geometry_logical: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme_mode: "dark".into(),
            scan_interval_ms: 2_000,
            window_width: 1_080,
            window_height: 720,
            window_x: None,
            window_y: None,
            window_geometry_logical: false,
        }
    }
}

impl UiSettings {
    pub fn normalized(mut self) -> Self {
        if !matches!(self.theme_mode.as_str(), "dark" | "light" | "system") {
            self.theme_mode = "dark".into();
        }
        self.scan_interval_ms = self.scan_interval_ms.clamp(500, 60_000);
        self.window_width = self.window_width.max(560);
        self.window_height = self.window_height.max(420);
        self
    }
}
