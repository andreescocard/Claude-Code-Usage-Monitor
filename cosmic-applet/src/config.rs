use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Persisted widget position, expressed as a top/left margin (in logical pixels)
/// from the top-left corner of the output. Wayland does not allow arbitrary
/// self-positioning, so the floating widget is anchored top-left and moved by
/// adjusting this margin.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WidgetConfig {
    pub x: i32,
    pub y: i32,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        // Start near the top-right-ish area; user drags from here.
        Self { x: 1200, y: 8 }
    }
}

fn config_path() -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("cosmic-applet-claude-usage");
    Some(dir.join("position.json"))
}

impl WidgetConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = config_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, content);
        }
    }
}
