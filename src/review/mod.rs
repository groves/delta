pub mod app;
pub mod diff_parser;
pub mod github;
pub mod hunk_id;
pub mod state;
pub mod tui;

use std::io::Cursor;

use anyhow::{Context, Result};
use bytelines::ByteLinesReader;

use crate::config::Config;

use self::app::ReviewHunk;
use self::diff_parser::FileDiff;

pub fn run(pr_number: u64, repo: Option<&str>, config: &Config, dry_run: bool) -> Result<()> {
    let metadata =
        github::fetch_pr_metadata(pr_number, repo).context("Failed to fetch PR metadata")?;

    let diff_text = github::fetch_pr_diff(pr_number, repo, &metadata.head_sha)
        .context("Failed to fetch PR diff")?;

    let file_diffs = diff_parser::parse_diff(&diff_text);

    let review_hunks = render_hunks(&file_diffs, config)?;

    if dry_run {
        eprintln!(
            "PR #{}: {} ({} hunks across {} files)",
            metadata.number,
            metadata.title,
            review_hunks.len(),
            file_diffs.len(),
        );
        return Ok(());
    }

    let viewed = state::load_viewed_state()?;

    let mut app = app::App::new(review_hunks, viewed, metadata);

    let pending = state::load_pending_comments(&app.pr_metadata.repo, app.pr_metadata.number)?;
    app.pending_comments = pending;

    tui::run_tui(&mut app)?;

    state::save_viewed_state(&app.viewed)?;
    state::save_pending_comments(
        &app.pr_metadata.repo,
        app.pr_metadata.number,
        &app.pending_comments,
    )?;

    Ok(())
}

pub fn run_local(config: &Config, dry_run: bool) -> Result<()> {
    let diff_text = github::fetch_local_diff().context("Failed to fetch local diff")?;

    let metadata = github::local_metadata().context("Failed to fetch local metadata")?;

    let file_diffs = diff_parser::parse_diff(&diff_text);

    let review_hunks = render_hunks(&file_diffs, config)?;

    if dry_run {
        eprintln!(
            "{} ({} hunks across {} files)",
            metadata.title,
            review_hunks.len(),
            file_diffs.len(),
        );
        return Ok(());
    }

    let viewed = state::load_viewed_state()?;

    let mut app = app::App::new(review_hunks, viewed, metadata);

    tui::run_tui(&mut app)?;

    state::save_viewed_state(&app.viewed)?;

    Ok(())
}

/// Strip OSC8 hyperlink sequences (]8;;...ST) that ansi-to-tui can't handle.
/// These have the form: ESC ] 8 ; params ; uri ST  ...text...  ESC ] 8 ; ; ST
/// where ST is ESC \ or BEL (\x07).
fn strip_osc_sequences(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Check for ESC ] (OSC start)
        if i + 1 < bytes.len() && bytes[i] == 0x1b && bytes[i + 1] == b']' {
            // Skip until ST (ESC \ or BEL)
            i += 2;
            while i < bytes.len() {
                if bytes[i] == 0x07 {
                    i += 1;
                    break;
                }
                if i + 1 < bytes.len() && bytes[i] == 0x1b && bytes[i + 1] == b'\\' {
                    i += 2;
                    break;
                }
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

pub(crate) fn is_lockfile(path: &str) -> bool {
    let basename = path.rsplit('/').next().unwrap_or(path);
    basename.ends_with(".lock") || basename == "package-lock.json" || basename == "pnpm-lock.yaml"
}

fn render_hunks(file_diffs: &[FileDiff], config: &Config) -> Result<Vec<ReviewHunk>> {
    let mut review_hunks = Vec::new();

    for file_diff in file_diffs {
        for hunk in &file_diff.hunks {
            let raw_segment = &hunk.raw_segment;
            let mut output = Cursor::new(Vec::new());

            let bytes = raw_segment.as_bytes().to_vec();
            let reader = Cursor::new(bytes);

            let result = crate::delta::delta(reader.byte_lines(), &mut output, config);
            let ansi_bytes = strip_osc_sequences(&output.into_inner());

            let rendered = if result.is_ok() && !ansi_bytes.is_empty() {
                match ansi_to_tui::IntoText::into_text(&ansi_bytes) {
                    Ok(text) => text,
                    Err(_) => {
                        ratatui::text::Text::raw(String::from_utf8_lossy(&ansi_bytes).to_string())
                    }
                }
            } else {
                ratatui::text::Text::raw(raw_segment.clone())
            };

            review_hunks.push(ReviewHunk {
                file_path: file_diff.path.clone(),
                content_hash: hunk.content_hash.clone(),
                plus_start: hunk.plus_start,
                rendered,
                raw_segment: hunk.raw_segment.clone(),
            });
        }
    }

    Ok(collapse_lockfile_hunks(review_hunks))
}

/// Merge consecutive hunks belonging to the same lockfile into a single hunk so the
/// reviewer doesn't have to step through dozens of collapsed lockfile summaries.
fn collapse_lockfile_hunks(hunks: Vec<ReviewHunk>) -> Vec<ReviewHunk> {
    let mut out: Vec<ReviewHunk> = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        let merge = is_lockfile(&hunk.file_path)
            && out
                .last()
                .map(|h| h.file_path == hunk.file_path)
                .unwrap_or(false);
        if merge {
            let last = out.last_mut().unwrap();
            last.rendered.lines.extend(hunk.rendered.lines);
            last.raw_segment.push_str(&hunk.raw_segment);
            last.content_hash = format!("{}+{}", last.content_hash, hunk.content_hash);
        } else {
            out.push(hunk);
        }
    }
    out
}
