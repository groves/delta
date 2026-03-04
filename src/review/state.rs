use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::app::PendingComment;

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

fn comments_file(repo: &str, pr_number: u64) -> PathBuf {
    let safe_name = repo.replace('/', "_");
    state_dir()
        .join("comments")
        .join(format!("{}_{}.json", safe_name, pr_number))
}

pub fn load_pending_comments(repo: &str, pr_number: u64) -> Result<Vec<PendingComment>> {
    let path = comments_file(repo, pr_number);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read pending comments from {}", path.display()))?;
    let comments: Vec<PendingComment> = serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse pending comments from {}", path.display()))?;
    Ok(comments)
}

pub fn save_pending_comments(
    repo: &str,
    pr_number: u64,
    comments: &[PendingComment],
) -> Result<()> {
    let path = comments_file(repo, pr_number);
    if comments.is_empty() {
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove {}", path.display()))?;
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let json =
        serde_json::to_string_pretty(&comments).context("Failed to serialize pending comments")?;
    fs::write(&path, json)
        .with_context(|| format!("Failed to write pending comments to {}", path.display()))?;
    Ok(())
}
