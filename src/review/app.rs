use std::collections::HashSet;

use ratatui::text::Text;
use serde::{Deserialize, Serialize};

use super::github::PrMetadata;

pub struct ReviewHunk {
    pub file_path: String,
    pub content_hash: String,
    pub plus_start: usize,
    pub rendered: Text<'static>,
    pub raw_segment: String,
}

#[derive(Serialize, Deserialize)]
pub struct PendingComment {
    pub path: String,
    pub line: usize,
    pub body: String,
}

pub struct App {
    pub hunks: Vec<ReviewHunk>,
    pub current_hunk: usize,
    pub scroll_offset: u16,
    pub viewed: HashSet<String>,
    /// Stack of hunk hashes the user has marked viewed, most recent last.
    /// Drives `undo_viewed`; excludes auto-marked hunks (e.g. lock files).
    pub viewed_history: Vec<String>,
    pub pr_metadata: PrMetadata,
    pub should_quit: bool,
    pub show_help: bool,
    /// Starting line offset of each hunk in the concatenated view.
    pub hunk_line_offsets: Vec<u16>,
    pub pending_comments: Vec<PendingComment>,
    /// Transient status message shown in the status bar; cleared on next key input.
    pub status_message: Option<String>,
    /// True after `r` is pressed; the next key either confirms (y/Y) or cancels.
    pub awaiting_revert_confirm: bool,
}

impl App {
    pub fn new(hunks: Vec<ReviewHunk>, viewed: HashSet<String>, metadata: PrMetadata) -> Self {
        let mut app = Self {
            hunks,
            current_hunk: 0,
            scroll_offset: 0,
            viewed,
            viewed_history: Vec::new(),
            pr_metadata: metadata,
            should_quit: false,
            show_help: false,
            hunk_line_offsets: Vec::new(),
            pending_comments: Vec::new(),
            status_message: None,
            awaiting_revert_confirm: false,
        };
        // Auto-mark lock files as viewed.
        for hunk in &app.hunks {
            if super::is_lockfile(&hunk.file_path) {
                app.viewed.insert(hunk.content_hash.clone());
            }
        }
        app.recompute_offsets();
        // Start at the first unviewed hunk.
        if let Some(i) = app.first_unviewed_hunk() {
            app.current_hunk = i;
            app.scroll_to_current_hunk();
        }
        app
    }

    fn first_unviewed_hunk(&self) -> Option<usize> {
        self.hunks
            .iter()
            .position(|h| !self.viewed.contains(&h.content_hash))
    }

    fn is_viewed(&self, hunk: &ReviewHunk) -> bool {
        self.viewed.contains(&hunk.content_hash)
    }

    /// Height of a hunk in the concatenated view: 1 line if collapsed (viewed), full height otherwise.
    fn hunk_display_height(&self, hunk: &ReviewHunk) -> u16 {
        if self.is_viewed(hunk) {
            1 // collapsed summary line
        } else {
            hunk.rendered.lines.len() as u16
        }
    }

    fn recompute_offsets(&mut self) {
        let mut offsets = Vec::with_capacity(self.hunks.len());
        let mut offset: u16 = 0;
        for hunk in &self.hunks {
            offsets.push(offset);
            let height = self.hunk_display_height(hunk);
            offset = offset.saturating_add(height).saturating_add(1); // +1 for separator
        }
        self.hunk_line_offsets = offsets;
    }

    pub fn current_hunk(&self) -> Option<&ReviewHunk> {
        self.hunks.get(self.current_hunk)
    }

    pub fn is_current_viewed(&self) -> bool {
        self.current_hunk()
            .map(|h| self.viewed.contains(&h.content_hash))
            .unwrap_or(false)
    }

    pub fn next_hunk(&mut self) {
        if self.current_hunk + 1 < self.hunks.len() {
            self.current_hunk += 1;
            self.scroll_to_current_hunk();
        }
    }

    pub fn prev_hunk(&mut self) {
        if self.current_hunk > 0 {
            self.current_hunk -= 1;
            self.scroll_to_current_hunk();
        }
    }

    fn scroll_to_current_hunk(&mut self) {
        if let Some(&offset) = self.hunk_line_offsets.get(self.current_hunk) {
            self.scroll_offset = offset;
        }
    }

    pub fn scroll_down(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Update current_hunk based on scroll position.
    pub fn update_current_hunk_from_scroll(&mut self) {
        for (i, &offset) in self.hunk_line_offsets.iter().enumerate().rev() {
            if self.scroll_offset >= offset {
                self.current_hunk = i;
                return;
            }
        }
    }

    pub fn toggle_viewed(&mut self) {
        if let Some(hunk) = self.hunks.get(self.current_hunk) {
            let hash = hunk.content_hash.clone();
            if self.viewed.contains(&hash) {
                // Marking as unviewed: expand and stay on current hunk.
                self.viewed.remove(&hash);
                self.viewed_history.retain(|h| h != &hash);
                self.recompute_offsets();
                self.scroll_to_current_hunk();
            } else {
                // Marking as viewed: collapse and advance to next unviewed hunk.
                self.viewed.insert(hash.clone());
                self.viewed_history.push(hash);
                self.recompute_offsets();
                if let Some(next) = self.hunks[self.current_hunk + 1..]
                    .iter()
                    .position(|h| !self.viewed.contains(&h.content_hash))
                {
                    self.current_hunk += 1 + next;
                }
                self.scroll_to_current_hunk();
            }
        }
    }

    /// Undo the most recent "mark viewed" action: un-mark that hunk, expand it,
    /// and navigate to it. Does nothing (beyond a status note) if the user
    /// hasn't marked anything viewed.
    pub fn undo_viewed(&mut self) {
        let Some(hash) = self.viewed_history.pop() else {
            self.status_message = Some("nothing to undo".to_string());
            return;
        };
        self.viewed.remove(&hash);
        if let Some(i) = self.hunks.iter().position(|h| h.content_hash == hash) {
            self.current_hunk = i;
        }
        self.recompute_offsets();
        self.scroll_to_current_hunk();
        if let Some(hunk) = self.current_hunk() {
            self.status_message = Some(format!("unviewed {}:{}", hunk.file_path, hunk.plus_start));
        }
    }

    pub fn open_in_editor(&self) {
        if let Some(hunk) = self.current_hunk() {
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
            let file = if let Some(root) = super::github::repo_root() {
                std::path::PathBuf::from(root)
                    .join(&hunk.file_path)
                    .to_string_lossy()
                    .to_string()
            } else {
                hunk.file_path.clone()
            };
            let line = self
                .topmost_visible_line()
                .unwrap_or_else(|| first_modified_line(hunk));

            let _ = std::process::Command::new(&editor)
                .arg(format!("+{}", line))
                .arg(file)
                .status();
        }
    }

    /// Rendered row index within the current hunk that corresponds to the
    /// topmost visible line. Returns `None` if there is no current hunk.
    fn row_in_current_hunk(&self) -> Option<usize> {
        self.current_hunk()?;
        let hunk_start = self
            .hunk_line_offsets
            .get(self.current_hunk)
            .copied()
            .unwrap_or(0);
        Some(self.scroll_offset.saturating_sub(hunk_start) as usize)
    }

    /// Map the current scroll position to a new-file line number within the
    /// current hunk. Walks `raw_segment` in lock-step with rendered rows under
    /// the assumption that delta produces ~one rendered row per raw line.
    /// Returns `None` if there is no current hunk.
    pub fn topmost_visible_line(&self) -> Option<usize> {
        let hunk = self.current_hunk()?;
        let plus_start = hunk.plus_start;
        let row_in_hunk = self.row_in_current_hunk()?;

        let mut line_num = plus_start;
        let mut past_at_at = false;
        for (idx, l) in hunk.raw_segment.lines().enumerate() {
            if idx >= row_in_hunk {
                return Some(line_num);
            }
            if !past_at_at {
                if l.starts_with("@@") {
                    past_at_at = true;
                }
                continue;
            }
            if l.starts_with('+') || l.starts_with(' ') {
                line_num += 1;
            }
            // '-' lines don't advance the new-file line counter.
        }
        Some(line_num.saturating_sub(1).max(plus_start))
    }

    /// Slice of `raw_segment` starting at the scrolled-to row. Pre-`@@` file
    /// headers are dropped, but the `@@` line itself is retained at the top so
    /// the result is still a recognizable diff fragment. If the user hasn't
    /// scrolled past the `@@` line, returns the full hunk.
    fn visible_raw_segment(&self) -> Option<String> {
        let hunk = self.current_hunk()?;
        let row_in_hunk = self.row_in_current_hunk()?;
        let lines: Vec<&str> = hunk.raw_segment.lines().collect();
        let at_at_idx = lines.iter().position(|l| l.starts_with("@@"));

        let first_content_idx = match at_at_idx {
            Some(i) => i + 1,
            None => 0,
        };

        if row_in_hunk <= first_content_idx {
            return Some(hunk.raw_segment.clone());
        }

        let content_start = row_in_hunk.min(lines.len());
        let mut out = String::new();
        if let Some(i) = at_at_idx {
            out.push_str(lines[i]);
            out.push('\n');
        }
        for l in &lines[content_start..] {
            out.push_str(l);
            out.push('\n');
        }
        Some(out.trim_end_matches('\n').to_string())
    }

    pub fn open_in_github(&self) {
        if self.pr_metadata.repo.is_empty() {
            return;
        }
        if let Some(hunk) = self.current_hunk() {
            let url = super::github::file_url(
                &self.pr_metadata.repo,
                self.pr_metadata.number,
                &hunk.file_path,
                hunk.plus_start,
            );
            let _ = super::github::open_in_browser(&url);
        }
    }

    /// Open $EDITOR with the hunk context and collect a comment for the pending review.
    /// Returns true if a comment was added.
    pub fn start_comment(&mut self) -> bool {
        if self.pr_metadata.repo.is_empty() || self.pr_metadata.head_sha.is_empty() {
            return false;
        }

        let hunk = match self.current_hunk() {
            Some(h) => h,
            None => return false,
        };
        let raw = &hunk.raw_segment;
        let file_path = hunk.file_path.clone();
        let plus_start = hunk.plus_start;

        // Extract body lines (skip diff --git, index, ---, +++, @@ headers)
        let body_lines: Vec<&str> = raw
            .lines()
            .skip_while(|l| {
                l.starts_with("diff --git ")
                    || l.starts_with("index ")
                    || l.starts_with("old mode ")
                    || l.starts_with("new mode ")
                    || l.starts_with("--- ")
                    || l.starts_with("+++ ")
                    || l.starts_with("@@")
                    || l.starts_with("similarity ")
                    || l.starts_with("rename ")
                    || l.starts_with("new file ")
                    || l.starts_with("deleted file ")
            })
            .collect();

        // Number each line: context and + lines get new-file line numbers, - lines get blank
        let mut numbered = String::new();
        let mut line_num = plus_start;
        for l in &body_lines {
            if l.starts_with('-') {
                numbered.push_str(&format!("    :{}\n", l));
            } else {
                numbered.push_str(&format!("{:>4}:{}\n", line_num, l));
                line_num += 1;
            }
        }

        // Write to temp file
        let tmp_dir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
        let tmp_path = format!("{}/drev-comment-{}", tmp_dir, std::process::id());
        if std::fs::write(&tmp_path, &numbered).is_err() {
            return false;
        }

        // Open $EDITOR
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let status = std::process::Command::new(&editor).arg(&tmp_path).status();

        let ok = matches!(status, Ok(s) if s.success());
        if !ok {
            let _ = std::fs::remove_file(&tmp_path);
            return false;
        }

        let contents = match std::fs::read_to_string(&tmp_path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let _ = std::fs::remove_file(&tmp_path);

        // Parse: find last diff line matching ^\s*(\d+):[+ ] — its number is target line.
        // Everything after the last diff line is the comment body.
        let mut last_diff_idx = None;
        let mut target_line = None;
        for (i, line) in contents.lines().enumerate() {
            let trimmed = line.trim_start();
            // Match lines like "42: context" or "42:+added" or "   :-removed"
            if let Some(colon_pos) = trimmed.find(':') {
                let before = &trimmed[..colon_pos];
                let after_colon = &trimmed[colon_pos + 1..];
                if after_colon.starts_with(' ')
                    || after_colon.starts_with('+')
                    || after_colon.starts_with('-')
                {
                    last_diff_idx = Some(i);
                    if let Ok(n) = before.parse::<usize>() {
                        target_line = Some(n);
                    }
                }
            }
        }

        let (last_diff_idx, target_line) = match (last_diff_idx, target_line) {
            (Some(i), Some(l)) => (i, l),
            _ => return false,
        };

        let comment_body: String = contents
            .lines()
            .skip(last_diff_idx + 1)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();

        if comment_body.is_empty() {
            return false;
        }

        self.pending_comments.push(PendingComment {
            path: file_path,
            line: target_line,
            body: comment_body,
        });

        true
    }

    /// Open $EDITOR for the user to type an instruction, then append the hunk's
    /// diff and the instruction to `COMMENTS.md` in the current directory so a
    /// separate Claude invocation can read and act on them. Non-blocking: drev
    /// stays open.
    pub fn ask_claude(&mut self) -> bool {
        let hunk = match self.current_hunk() {
            Some(h) => h,
            None => return false,
        };
        let raw_segment = self
            .visible_raw_segment()
            .unwrap_or_else(|| hunk.raw_segment.clone());
        let file_path = hunk.file_path.clone();
        let target_line = self.topmost_visible_line().unwrap_or(hunk.plus_start);

        // Template: each diff line is prefixed with `> ` so the user can delete
        // any lines they don't want saved to COMMENTS.md (target a subset).
        // Helper text uses `#` and is stripped from both the diff and the
        // instruction.
        let mut template = String::new();
        template.push_str(&format!("# Hunk from {}:{}\n", file_path, target_line));
        template.push_str(
            "# Lines starting with `> ` below are the diff. Delete any you\n\
             # don't want saved; keep what you want Claude to focus on.\n",
        );
        for line in raw_segment.lines() {
            template.push_str("> ");
            template.push_str(line);
            template.push('\n');
        }
        template.push_str(
            "#\n# Write your instruction for Claude below. Lines starting with `#`\n\
             # or `> ` are ignored from the instruction.\n\n",
        );

        let tmp_dir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
        let tmp_path = format!("{}/drev-claude-{}", tmp_dir, std::process::id());
        if std::fs::write(&tmp_path, &template).is_err() {
            return false;
        }

        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let status = std::process::Command::new(&editor).arg(&tmp_path).status();
        if !matches!(status, Ok(s) if s.success()) {
            let _ = std::fs::remove_file(&tmp_path);
            return false;
        }

        let contents = match std::fs::read_to_string(&tmp_path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let _ = std::fs::remove_file(&tmp_path);

        let kept_diff: String = contents
            .lines()
            .filter_map(strip_quote_prefix)
            .collect::<Vec<_>>()
            .join("\n");

        let instruction: String = contents
            .lines()
            .filter(|l| !l.trim_start().starts_with('#') && strip_quote_prefix(l).is_none())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();

        if instruction.is_empty() {
            self.status_message = Some("ask claude: no instruction given".to_string());
            return false;
        }

        let comment_id = next_comment_id();
        let comment_filename = format!("COMMENT-{}.md", comment_id);
        let comment_path = std::path::PathBuf::from(&comment_filename);

        let mut entry = String::new();
        entry.push_str(&format!("## `{}:{}`\n\n", file_path, target_line));
        entry.push_str("```diff\n");
        entry.push_str(kept_diff.trim_end_matches('\n'));
        entry.push_str("\n```\n\n");
        entry.push_str("### Request\n\n");
        entry.push_str(&instruction);
        entry.push('\n');

        if std::fs::write(&comment_path, &entry).is_err() {
            self.status_message = Some(format!("ask claude: failed to write {}", comment_filename));
            return false;
        }

        let claude_prompt = claude_watch_prompt();
        let cwd = std::env::current_dir().ok();
        let command = match cwd.as_ref().and_then(|p| p.to_str()) {
            Some(dir) => format!(
                "cd {} && ,c {}\n",
                shell_single_quote(dir),
                shell_single_quote(&claude_prompt),
            ),
            None => format!(",c {}\n", shell_single_quote(&claude_prompt)),
        };

        if copy_to_clipboard(&command) {
            self.status_message = Some(format!(
                "wrote {} ({}); claude command copied — paste in a new terminal if not already running",
                comment_filename, file_path
            ));
        } else {
            self.status_message = Some(format!(
                "wrote {} ({}) (clipboard copy failed)",
                comment_filename, file_path
            ));
        }
        true
    }

    /// Submit all pending comments as a single GitHub review.
    /// Returns the review URL on success.
    pub fn submit_review(&mut self) -> Option<String> {
        if self.pending_comments.is_empty() {
            return None;
        }

        let url = super::github::create_pr_review(
            &self.pr_metadata.repo,
            self.pr_metadata.number,
            &self.pr_metadata.head_sha,
            &self.pending_comments,
        )
        .ok()?;

        self.pending_comments.clear();
        let _ = super::github::open_in_browser(&url);
        Some(url)
    }

    /// Whether the diff being reviewed comes from a remote PR (vs the local working copy).
    pub fn is_pr_mode(&self) -> bool {
        !self.pr_metadata.repo.is_empty()
    }

    /// Begin the revert-hunk flow. The status bar will show a y/N prompt; the
    /// next key press either calls `confirm_revert` or `cancel_revert`. No-op
    /// in PR mode.
    pub fn request_revert(&mut self) {
        if self.is_pr_mode() || self.current_hunk().is_none() {
            return;
        }
        self.awaiting_revert_confirm = true;
    }

    pub fn cancel_revert(&mut self) {
        self.awaiting_revert_confirm = false;
    }

    /// Apply the current hunk's diff in reverse against the working copy via
    /// `git apply -R`. On success, drops the hunk from the in-memory list.
    pub fn confirm_revert(&mut self) {
        self.awaiting_revert_confirm = false;
        let Some(hunk) = self.current_hunk() else {
            return;
        };
        let patch = hunk.raw_segment.clone();
        let path_display = hunk.file_path.clone();

        let tmp_dir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
        let tmp_path = format!("{}/drev-revert-{}.patch", tmp_dir, std::process::id());
        if let Err(e) = std::fs::write(&tmp_path, &patch) {
            self.status_message = Some(format!("revert: failed to write patch: {}", e));
            return;
        }

        let cwd = super::github::repo_root().unwrap_or_else(|| ".".to_string());
        let output = std::process::Command::new("git")
            .args(["apply", "-R", "--recount"])
            .arg(&tmp_path)
            .current_dir(&cwd)
            .output();
        let _ = std::fs::remove_file(&tmp_path);

        match output {
            Ok(out) if out.status.success() => {
                let idx = self.current_hunk;
                self.hunks.remove(idx);
                self.recompute_offsets();
                if self.hunks.is_empty() {
                    self.current_hunk = 0;
                } else if self.current_hunk >= self.hunks.len() {
                    self.current_hunk = self.hunks.len() - 1;
                }
                self.scroll_to_current_hunk();
                self.status_message = Some(format!("reverted hunk in {}", path_display));
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let msg = if stderr.is_empty() {
                    "revert: git apply -R failed".to_string()
                } else {
                    format!("revert failed: {}", stderr)
                };
                self.status_message = Some(msg);
            }
            Err(e) => {
                self.status_message = Some(format!("revert: failed to run git apply: {}", e));
            }
        }
    }

    pub fn viewed_count(&self) -> usize {
        self.hunks
            .iter()
            .filter(|h| self.viewed.contains(&h.content_hash))
            .count()
    }
}

/// Strip the editor-template `> ` prefix used to mark diff lines. Returns the
/// remaining content if the line is a diff line (`> ...` or a bare `>`), or
/// `None` for any other line (helper text or instruction).
fn strip_quote_prefix(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("> ") {
        Some(rest)
    } else if line == ">" {
        Some("")
    } else {
        None
    }
}

/// Shell command (POSIX `sh`) that watches the cwd for `COMMENT-*.md` files and
/// prints one line per file as it appears, re-printing a name if the file is
/// deleted and recreated. It is meant to run under Claude's Monitor tool so each
/// printed line becomes an event that wakes the agent.
///
/// It polls (1s) rather than using a native filesystem-event watcher
/// (watchexec/fswatch/inotifywait). Under macOS Seatbelt — which sandboxes
/// Claude Code's Bash tool — the FSEvents/kqueue APIs those tools rely on are
/// starved: they register no watches and emit nothing, failing *silently*. That
/// silent failure is what defeated the earlier watchexec-based prompts. `stat()`
/// polling is unaffected by the sandbox, so it works where the feature runs.
///
/// The two leading `setopt`/`shopt` lines enable nullglob (zsh and bash
/// respectively; each silenced so the wrong-shell one is a harmless no-op).
/// Without them an unmatched `COMMENT-*.md` glob is fatal under zsh — it errors
/// with "no matches found" and the loop dies with exit 1 the instant the
/// directory holds no comment files (e.g. right after the last one is deleted).
/// nullglob makes the glob expand to nothing, so the loop just idles.
///
/// Kept in sync with the prompt via [`claude_watch_prompt`]; exercised by the
/// `comment_watch_command_emits_on_landing` test.
const COMMENT_WATCH_COMMAND: &str = r#"setopt NULL_GLOB 2>/dev/null
shopt -s nullglob 2>/dev/null
last=""
while true; do
  cur=""
  for f in COMMENT-*.md; do
    [ -e "$f" ] || continue
    cur="$cur $f"
  done
  for f in $cur; do
    case " $last " in
      *" $f "*) ;;
      *) echo "$f" ;;
    esac
  done
  last="$cur"
  sleep 1
done"#;

/// Build the prompt drev hands to a separate `claude` invocation so it watches
/// for and processes `COMMENT-*.md` files as the user drops them. Embeds
/// [`COMMENT_WATCH_COMMAND`] verbatim so the prompt and the tested command never
/// drift apart.
fn claude_watch_prompt() -> String {
    format!(
        r#"Process COMMENT-<id>.md files in this directory (e.g. COMMENT-1.md, COMMENT-2.md). Each file contains a diff hunk and a `### Request` section.

A separate program writes these files as the user reviews code, so they appear over time and you must react to each as it lands. Do NOT use a native filesystem-event watcher (watchexec, fswatch, inotifywait): the Bash sandbox starves the FSEvents/kqueue APIs they rely on, so they register nothing and emit nothing — they fail silently. Use the polling watcher below.

Each COMMENT is one of two kinds; read its `### Request` to classify it:
  - QUESTION — it only asks for information or an explanation; answering it does not modify any file in the repo.
  - EDIT — fulfilling it requires changing code or other repo files.

Dispatch policy:
  - QUESTIONs run immediately and in parallel. Dispatch a subagent the moment one lands; there is no limit on how many questions run at once, and they may run alongside an edit.
  - EDITs run strictly one at a time. Keep a FIFO queue of pending edits and hold the invariant that AT MOST ONE edit subagent is ever in flight — two agents editing concurrently can clobber each other's changes. When an edit subagent finishes, start the next queued edit. (Questions don't touch repo files, so they never count against this limit.)

Order matters — start the watcher BEFORE the initial scan so files that land during startup aren't lost in a race.

Step 1: start the watcher with the Monitor tool (persistent: true). First write this exact script to /tmp/claude/drev-comment-watch.sh (create /tmp/claude if it does not exist), then have the Monitor tool run `sh /tmp/claude/drev-comment-watch.sh`. Writing it to a file and invoking the interpreter on that file — rather than passing the multi-line command inline — keeps the Bash sandbox from prompting for approval. The script:
{COMMENT_WATCH_COMMAND}
It polls the cwd once a second and prints one line per COMMENT-*.md filename as it appears (re-printing a name if the file is deleted and recreated). Each printed line is a Monitor event that wakes you; it stays quiet when nothing changes.

Step 2: list the COMMENT-*.md already present, read and classify each, and route it per the dispatch policy (fire questions now; append edits to the queue and start the first one). Track handled paths in a set — call it `dispatched`.

Step 3: handle Monitor events. Each event is a single line naming a COMMENT-*.md file. For each line:
  - if the path is already in `dispatched`, skip it (already handled or in flight);
  - otherwise add it to `dispatched`, read and classify it, route it per the dispatch policy, and remove it from `dispatched` once its subagent finishes.
  This prevents the double-dispatch at startup — the initial scan and the watcher's first poll both surface pre-existing files — while still letting a recycled filename (deleted then recreated) be picked up.

Keep handling events indefinitely; do not poll or re-glob the directory yourself between events — the watcher is authoritative.

If the watcher stops (its Monitor task exits or is reported stopped), restart it once by re-running `sh /tmp/claude/drev-comment-watch.sh` and resume; if it stops again in quick succession, stop and report.

Subagent rules: give the subagent the file path and tell it to read the file and address the request:
  - EDIT subagents: make the changes, then delete the COMMENT file when done. Report what you changed.
  - QUESTION subagents: append a `### Response` section with the answer to the file, leave the file in place, and return the answer.

After a QUESTION subagent finishes, print the question (the text of its `### Request`) and the answer here in this session, so the user sees the Q&A in the terminal as well as in the file."#
    )
}

/// Pick the next unused id for a `COMMENT-<id>.md` file in the cwd.
fn next_comment_id() -> u32 {
    let mut max_id = 0u32;
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(rest) = name.strip_prefix("COMMENT-") else {
                continue;
            };
            let Some(num_str) = rest.strip_suffix(".md") else {
                continue;
            };
            if let Ok(n) = num_str.parse::<u32>() {
                if n > max_id {
                    max_id = n;
                }
            }
        }
    }
    max_id + 1
}

/// POSIX single-quote a string: wrap in `'…'`, escaping any `'` as `'\''`.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Pipe `text` into `pbcopy`. Returns true on success.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = match Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(text.as_bytes()).is_err()
    {
        return false;
    }
    matches!(child.wait(), Ok(s) if s.success())
}

/// Find the line number of the first `+` (added) line in the hunk's raw diff.
/// Falls back to `plus_start` if there are no additions.
fn first_modified_line(hunk: &ReviewHunk) -> usize {
    let mut line_num = hunk.plus_start;
    let in_body = hunk
        .raw_segment
        .lines()
        .skip_while(|l| !l.starts_with("@@"))
        .skip(1); // skip the @@ line itself

    for l in in_body {
        if l.starts_with('+') {
            return line_num;
        } else if l.starts_with('-') {
            // deletions don't advance the new-file line counter
        } else {
            line_num += 1; // context line
        }
    }
    hunk.plus_start
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::{Line, Text};
    use std::time::{Duration, Instant};

    fn make_hunk(path: &str, hash: &str, num_lines: usize) -> ReviewHunk {
        let lines: Vec<Line<'static>> = (0..num_lines)
            .map(|i| Line::from(format!("line {i}")))
            .collect();
        ReviewHunk {
            file_path: path.to_string(),
            content_hash: hash.to_string(),
            plus_start: 1,
            rendered: Text::from(lines),
            raw_segment: String::new(),
        }
    }

    fn make_metadata() -> PrMetadata {
        PrMetadata {
            number: 1,
            title: "test".to_string(),
            repo: "test/repo".to_string(),
            head_sha: String::new(),
        }
    }

    fn make_local_metadata() -> PrMetadata {
        PrMetadata {
            number: 0,
            title: "local".to_string(),
            repo: String::new(),
            head_sha: String::new(),
        }
    }

    #[test]
    fn new_starts_at_first_unviewed() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 5),
            make_hunk("b.rs", "h2", 5),
            make_hunk("c.rs", "h3", 5),
        ];
        let viewed: HashSet<String> = ["h1".to_string(), "h2".to_string()].into();
        let app = App::new(hunks, viewed, make_metadata());
        assert_eq!(app.current_hunk, 2, "should start at first unviewed hunk");
    }

    #[test]
    fn new_starts_at_zero_when_none_viewed() {
        let hunks = vec![make_hunk("a.rs", "h1", 5), make_hunk("b.rs", "h2", 5)];
        let app = App::new(hunks, HashSet::new(), make_metadata());
        assert_eq!(app.current_hunk, 0);
    }

    #[test]
    fn offsets_account_for_collapsed_hunks() {
        let hunks = vec![make_hunk("a.rs", "h1", 10), make_hunk("b.rs", "h2", 10)];
        // h1 is viewed (collapsed = 1 line), h2 is not (10 lines).
        let viewed: HashSet<String> = ["h1".to_string()].into();
        let app = App::new(hunks, viewed, make_metadata());
        // hunk 0: offset=0, height=1 (collapsed), separator=1 → next at 2
        // hunk 1: offset=2
        assert_eq!(app.hunk_line_offsets, vec![0, 2]);
    }

    #[test]
    fn offsets_all_expanded() {
        let hunks = vec![make_hunk("a.rs", "h1", 10), make_hunk("b.rs", "h2", 10)];
        let app = App::new(hunks, HashSet::new(), make_metadata());
        // hunk 0: offset=0, height=10, separator=1 → next at 11
        // hunk 1: offset=11
        assert_eq!(app.hunk_line_offsets, vec![0, 11]);
    }

    #[test]
    fn toggle_viewed_marks_and_advances() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
            make_hunk("c.rs", "h3", 10),
        ];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());
        assert_eq!(app.current_hunk, 0);

        app.toggle_viewed();

        assert!(app.viewed.contains("h1"), "h1 should be marked viewed");
        assert_eq!(app.current_hunk, 1, "should advance to next hunk");
        // Offsets should reflect h1 collapsed (1 line) instead of 10.
        assert_eq!(app.hunk_line_offsets, vec![0, 2, 13]);
    }

    #[test]
    fn toggle_viewed_skips_already_viewed() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
            make_hunk("c.rs", "h3", 10),
            make_hunk("d.rs", "h4", 10),
        ];
        // h2 and h3 are already viewed.
        let viewed: HashSet<String> = ["h2".to_string(), "h3".to_string()].into();
        let mut app = App::new(hunks, viewed, make_metadata());
        assert_eq!(app.current_hunk, 0);

        app.toggle_viewed();

        assert!(app.viewed.contains("h1"));
        assert_eq!(
            app.current_hunk, 3,
            "should skip viewed h2, h3 and land on h4"
        );
    }

    #[test]
    fn toggle_viewed_stays_when_all_remaining_viewed() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
            make_hunk("c.rs", "h3", 10),
        ];
        let viewed: HashSet<String> = ["h2".to_string(), "h3".to_string()].into();
        let mut app = App::new(hunks, viewed, make_metadata());
        assert_eq!(app.current_hunk, 0);

        app.toggle_viewed();

        assert!(app.viewed.contains("h1"));
        assert_eq!(
            app.current_hunk, 0,
            "should stay when no unviewed hunks remain after"
        );
    }

    #[test]
    fn toggle_unview_expands_and_stays() {
        let hunks = vec![make_hunk("a.rs", "h1", 10), make_hunk("b.rs", "h2", 10)];
        let viewed: HashSet<String> = ["h1".to_string()].into();
        let mut app = App::new(hunks, viewed, make_metadata());
        // Navigate to hunk 0 (which is viewed/collapsed).
        app.current_hunk = 0;
        app.scroll_to_current_hunk();

        app.toggle_viewed();

        assert!(!app.viewed.contains("h1"), "h1 should be unviewed");
        assert_eq!(app.current_hunk, 0, "should stay on current hunk");
        // Offsets should now reflect h1 expanded (10 lines).
        assert_eq!(app.hunk_line_offsets, vec![0, 11]);
    }

    #[test]
    fn undo_viewed_unmarks_last_and_navigates_back() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
            make_hunk("c.rs", "h3", 10),
        ];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());

        // Mark h1, then h2 viewed; lands on h3.
        app.toggle_viewed();
        app.toggle_viewed();
        assert_eq!(app.current_hunk, 2);
        assert!(app.viewed.contains("h1") && app.viewed.contains("h2"));

        // Undo restores the most recent (h2) and navigates back to it.
        app.undo_viewed();
        assert!(!app.viewed.contains("h2"), "h2 should be unviewed");
        assert!(app.viewed.contains("h1"), "h1 should still be viewed");
        assert_eq!(app.current_hunk, 1, "should navigate back to h2");

        // Undo again restores h1.
        app.undo_viewed();
        assert!(!app.viewed.contains("h1"), "h1 should be unviewed");
        assert_eq!(app.current_hunk, 0);
    }

    #[test]
    fn undo_viewed_is_noop_when_nothing_marked() {
        let hunks = vec![make_hunk("a.rs", "h1", 10), make_hunk("b.rs", "h2", 10)];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());

        app.undo_viewed();

        assert!(app.viewed.is_empty());
        assert_eq!(app.current_hunk, 0);
    }

    #[test]
    fn undo_viewed_ignores_manually_unviewed_hunks() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
            make_hunk("c.rs", "h3", 10),
        ];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());

        // Mark h1 then h2 viewed.
        app.toggle_viewed();
        app.toggle_viewed();

        // Manually unview h1 (toggling a viewed hunk off should drop it from history).
        app.current_hunk = 0;
        app.toggle_viewed();
        assert!(!app.viewed.contains("h1"));

        // Undo should restore h2 (the only remaining marked hunk), not revisit h1.
        app.undo_viewed();
        assert!(!app.viewed.contains("h2"), "h2 should be unviewed");
        assert_eq!(app.current_hunk, 1, "should navigate to h2");

        // Nothing left to undo.
        app.undo_viewed();
        assert!(app.viewed.is_empty());
    }

    #[test]
    fn undo_viewed_does_not_unview_auto_marked_lockfiles() {
        let hunks = vec![
            make_hunk("Cargo.lock", "lock", 10),
            make_hunk("a.rs", "h1", 10),
        ];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());
        // Lockfile auto-marked viewed; review starts on the first unviewed hunk.
        assert!(app.viewed.contains("lock"));
        assert_eq!(app.current_hunk, 1);

        // Undo with no user-marked hunks must not touch the auto-marked lockfile.
        app.undo_viewed();
        assert!(
            app.viewed.contains("lock"),
            "auto-marked lockfile should remain viewed"
        );
    }

    /// Simulate what draw_diff does: build the line list and verify
    /// that viewed hunks produce a single collapsed line.
    #[test]
    fn draw_produces_collapsed_line_for_viewed_hunk() {
        let hunks = vec![make_hunk("a.rs", "h1", 10), make_hunk("b.rs", "h2", 10)];
        let viewed: HashSet<String> = ["h1".to_string()].into();
        let app = App::new(hunks, viewed, make_metadata());

        // Simulate draw_diff line building.
        let mut lines: Vec<String> = Vec::new();
        for (i, hunk) in app.hunks.iter().enumerate() {
            if i > 0 {
                lines.push("separator".to_string());
            }
            let is_viewed = app.viewed.contains(&hunk.content_hash);
            if is_viewed {
                lines.push(format!("[viewed] {}:{}", hunk.file_path, hunk.plus_start));
            } else {
                for line in &hunk.rendered.lines {
                    lines.push(format!("{line}"));
                }
            }
        }

        // Hunk 0 is viewed → 1 collapsed line, then separator, then hunk 1 → 10 lines.
        // Total = 1 + 1 + 10 = 12.
        assert_eq!(lines.len(), 12);
        assert!(
            lines[0].contains("[viewed]"),
            "first line should be collapsed summary"
        );
        assert_eq!(lines[1], "separator");
    }

    #[test]
    fn request_revert_is_noop_in_pr_mode() {
        let hunks = vec![make_hunk("a.rs", "h1", 3)];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());
        app.request_revert();
        assert!(
            !app.awaiting_revert_confirm,
            "PR mode should not arm the revert prompt"
        );
    }

    #[test]
    fn request_revert_arms_prompt_in_local_mode() {
        let hunks = vec![make_hunk("a.rs", "h1", 3)];
        let mut app = App::new(hunks, HashSet::new(), make_local_metadata());
        app.request_revert();
        assert!(app.awaiting_revert_confirm);
    }

    #[test]
    fn cancel_revert_clears_prompt() {
        let hunks = vec![make_hunk("a.rs", "h1", 3)];
        let mut app = App::new(hunks, HashSet::new(), make_local_metadata());
        app.request_revert();
        app.cancel_revert();
        assert!(!app.awaiting_revert_confirm);
        // Hunk should still be present.
        assert_eq!(app.hunks.len(), 1);
    }

    #[test]
    fn request_revert_with_no_hunks_is_noop() {
        let mut app = App::new(Vec::new(), HashSet::new(), make_local_metadata());
        app.request_revert();
        assert!(!app.awaiting_revert_confirm);
    }

    /// Create a unique, empty temp directory (no `tempfile` dev-dep in this crate).
    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut p = std::env::temp_dir();
        p.push(format!("drev-test-{tag}-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&p).expect("create temp dir");
        p
    }

    /// Block until a line equal to `want` arrives, or `timeout` elapses.
    fn wait_for_line(
        rx: &std::sync::mpsc::Receiver<String>,
        want: &str,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(line) if line.trim() == want => return true,
                Ok(_) => continue,
                Err(_) => return false, // timeout or sender hung up
            }
        }
        false
    }

    /// Regression test for the nullglob prefix: under zsh an unmatched
    /// `COMMENT-*.md` glob is fatal ("no matches found", exit 1), so without the
    /// prefix the loop dies the instant the directory empties — e.g. right after
    /// the last comment file is deleted. Run the shipped command under zsh, empty
    /// the directory mid-flight, and assert it still reports a file that lands
    /// afterward. Skips if zsh is not installed (e.g. minimal CI).
    #[cfg(unix)]
    #[test]
    fn comment_watch_command_survives_empty_dir_under_zsh() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::sync::mpsc;

        let zsh_available = Command::new("zsh")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !zsh_available {
            eprintln!("skipping: zsh not installed");
            return;
        }

        let dir = unique_temp_dir("watch-zsh");
        std::fs::write(dir.join("COMMENT-1.md"), "## pre\n### Request\nq\n").unwrap();

        let mut child = Command::new("zsh")
            .arg("-c")
            .arg(COMMENT_WATCH_COMMAND)
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn watcher");

        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let emitted_preexisting = wait_for_line(&rx, "COMMENT-1.md", Duration::from_secs(5));

        // Empty the directory — the moment the original (prefix-less) loop dies
        // under zsh — and give the poll loop time to glob an empty cwd.
        std::fs::remove_file(dir.join("COMMENT-1.md")).unwrap();
        std::thread::sleep(Duration::from_secs(2));

        // If the loop survived the empty glob, a file landing now still emits.
        std::fs::write(dir.join("COMMENT-2.md"), "## new\n### Request\nq\n").unwrap();
        let emitted_after_empty = wait_for_line(&rx, "COMMENT-2.md", Duration::from_secs(5));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            emitted_preexisting,
            "watcher did not emit a pre-existing COMMENT-*.md within 5s"
        );
        assert!(
            emitted_after_empty,
            "watcher died on an empty directory: a COMMENT-*.md that landed after the dir emptied was not reported (nullglob prefix missing or broken)"
        );
    }

    /// The core contract: the watcher command drev ships must emit a line naming
    /// a `COMMENT-*.md` file both when one already exists at startup and — the
    /// part every native-FS-event iteration silently failed — when one lands
    /// *after* the watcher is running. Deterministic and fast; runs in `cargo test`.
    #[cfg(unix)]
    #[test]
    fn comment_watch_command_emits_on_landing() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::sync::mpsc;

        let dir = unique_temp_dir("watch");

        // Pre-existing file: the watcher is started before drev's own initial
        // scan, so its first poll must surface files already on disk.
        std::fs::write(dir.join("COMMENT-1.md"), "## pre\n### Request\nq\n").unwrap();

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(COMMENT_WATCH_COMMAND)
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn watcher");

        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let emitted_preexisting = wait_for_line(&rx, "COMMENT-1.md", Duration::from_secs(5));

        // The behavior the whole feature hangs on: a file that lands while the
        // watcher is already running must produce an event.
        std::fs::write(dir.join("COMMENT-2.md"), "## new\n### Request\nq\n").unwrap();
        let emitted_landed = wait_for_line(&rx, "COMMENT-2.md", Duration::from_secs(5));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            emitted_preexisting,
            "watcher did not emit a pre-existing COMMENT-*.md within 5s"
        );
        assert!(
            emitted_landed,
            "watcher did not emit a COMMENT-*.md that landed after startup within 5s"
        );
    }

    /// End-to-end: launch a real headless `claude` with the actual shipped
    /// prompt, drop a question-COMMENT after it starts, and assert the agent
    /// picks it up and appends a `### Response`. Ignored by default — it needs a
    /// `claude` binary (with the Monitor and Agent tools), network, and costs
    /// tokens. Run with:
    ///   cargo test -p git-delta claude_processes_landed_comment_end_to_end \
    ///     -- --ignored --nocapture
    /// Override the binary with CLAUDE_BIN=/path/to/claude.
    #[cfg(unix)]
    #[test]
    #[ignore = "spawns a real claude process; run manually with --ignored"]
    fn claude_processes_landed_comment_end_to_end() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let claude = std::env::var("CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string());
        let available = Command::new(&claude)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !available {
            eprintln!("skipping: `{claude}` not runnable (set CLAUDE_BIN to override)");
            return;
        }

        let dir = unique_temp_dir("e2e");
        let log_path = dir.join("claude.log");
        let log = std::fs::File::create(&log_path).unwrap();

        // The exact prompt drev hands out.
        let prompt = claude_watch_prompt();

        let mut child = Command::new(&claude)
            .args(["--print", "--effort", "high", "--dangerously-skip-permissions"])
            .arg(&prompt)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            // Fall back to the persisted interactive login; an invalid
            // ANTHROPIC_API_KEY in the env (e.g. one injected by an outer Claude
            // session) otherwise makes `claude` exit immediately with
            // "Invalid API key".
            .env_remove("ANTHROPIC_API_KEY")
            .process_group(0) // own group so we can tear down the whole tree
            .spawn()
            .expect("spawn claude");
        let pgid = child.id() as i32;

        // Let the watcher come up before dropping a file.
        std::thread::sleep(Duration::from_secs(5));

        // A question request. The signal that the file was genuinely dispatched
        // is the *answer* (7 × 8 = 56) appearing — a token absent from both the
        // request text and the diff, so it can only get there if the agent ran.
        // (Checking for the literal "### Response" marker would false-positive,
        // since the agent is instructed to write that heading and we'd have to
        // mention it in the request.)
        const ANSWER: &str = "56";
        let comment = "## `demo.txt:1`\n\n```diff\n@@ -1 +1 @@\n-foo\n+bar\n```\n\n### Request\n\nConnectivity test — do not edit any file except this one. Append the numeric answer to: what is 7 times 8?\n";
        assert!(
            !comment.contains(ANSWER),
            "request text must not contain the answer token, or detection is meaningless"
        );
        std::fs::write(dir.join("COMMENT-1.md"), comment).unwrap();

        let deadline = Instant::now() + Duration::from_secs(240);
        let mut got_response = false;
        while Instant::now() < deadline {
            if let Ok(s) = std::fs::read_to_string(dir.join("COMMENT-1.md"))
                && s != comment
                && s.contains(ANSWER)
            {
                got_response = true;
                break;
            }
            std::thread::sleep(Duration::from_secs(2));
        }

        // Best-effort teardown of the whole process group (silence the expected
        // "No such process" once claude has already exited).
        let kill = |sig: &str| {
            let _ = Command::new("kill")
                .arg(sig)
                .arg(format!("-{pgid}"))
                .stderr(Stdio::null())
                .status();
        };
        kill("-TERM");
        let _ = child.kill();
        let _ = child.wait();
        kill("-KILL");

        if !got_response {
            let tail = std::fs::read_to_string(&log_path).unwrap_or_default();
            let _ = std::fs::remove_dir_all(&dir);
            panic!(
                "claude did not append the answer ({ANSWER}) within 240s.\n--- claude.log ---\n{tail}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
