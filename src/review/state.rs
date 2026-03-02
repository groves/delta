use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct ViewedState {
    viewed_hunks: Vec<String>,
}

fn state_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg).join("delta").join("reviews")
    } else if let Some(home) = dirs::home_dir() {
        home.join(".local")
            .join("share")
            .join("delta")
            .join("reviews")
    } else {
        PathBuf::from(".delta").join("reviews")
    }
}

fn state_file() -> PathBuf {
    state_dir().join("viewed.json")
}

pub fn load_viewed_state() -> Result<HashSet<String>> {
    let path = state_file();

    if !path.exists() {
        return Ok(HashSet::new());
    }

    let data = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read review state from {}", path.display()))?;

    let state: ViewedState = serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse review state from {}", path.display()))?;

    Ok(state.viewed_hunks.into_iter().collect())
}

pub fn save_viewed_state(viewed: &HashSet<String>) -> Result<()> {
    let path = state_file();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    let state = ViewedState {
        viewed_hunks: viewed.iter().cloned().collect(),
    };

    let json = serde_json::to_string_pretty(&state).context("Failed to serialize review state")?;
    fs::write(&path, json)
        .with_context(|| format!("Failed to write review state to {}", path.display()))?;

    Ok(())
}
