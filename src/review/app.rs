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
    pub pr_metadata: PrMetadata,
    pub should_quit: bool,
    pub show_help: bool,
    /// Starting line offset of each hunk in the concatenated view.
    pub hunk_line_offsets: Vec<u16>,
    pub pending_comments: Vec<PendingComment>,
    /// Transient status message shown in the status bar; cleared on next key input.
    pub status_message: Option<String>,
}

impl App {
    pub fn new(hunks: Vec<ReviewHunk>, viewed: HashSet<String>, metadata: PrMetadata) -> Self {
        let mut app = Self {
            hunks,
            current_hunk: 0,
            scroll_offset: 0,
            viewed,
            pr_metadata: metadata,
            should_quit: false,
            show_help: false,
            hunk_line_offsets: Vec::new(),
            pending_comments: Vec::new(),
            status_message: None,
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
                self.recompute_offsets();
                self.scroll_to_current_hunk();
            } else {
                // Marking as viewed: collapse and advance to next unviewed hunk.
                self.viewed.insert(hash);
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

        let comments_path = std::path::PathBuf::from("COMMENTS.md");
        let file_existed = comments_path.exists();

        let mut entry = String::new();
        if !file_existed {
            entry.push_str(
                "<!--\n\
                 This file accumulates diff hunks from a PR review along with a question\n\
                 or request about each hunk for Claude to address. Each entry below has:\n\
                   * a heading with the file path and starting line\n\
                   * a fenced ```diff block containing the hunk\n\
                   * a `### Request` section with the user's instruction\n\
                 Entries are separated by a `---` horizontal rule.\n\
                 -->\n\n",
            );
        } else {
            entry.push_str("\n---\n\n");
        }
        entry.push_str(&format!("## `{}:{}`\n\n", file_path, target_line));
        entry.push_str("```diff\n");
        entry.push_str(kept_diff.trim_end_matches('\n'));
        entry.push_str("\n```\n\n");
        entry.push_str("### Request\n\n");
        entry.push_str(&instruction);
        entry.push('\n');

        use std::io::Write;
        let write_result = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&comments_path)
            .and_then(|mut f| f.write_all(entry.as_bytes()));

        if write_result.is_err() {
            self.status_message =
                Some("ask claude: failed to write COMMENTS.md".to_string());
            return false;
        }

        let claude_prompt = "Read COMMENTS.md in this directory. Each entry contains a \
                             diff hunk followed by a `### Request` section. Work through \
                             every entry in order, addressing the request — make the code \
                             changes (or answer the question) and remove the entry from \
                             COMMENTS.md once handled.";
        let cwd = std::env::current_dir().ok();
        let command = match cwd.as_ref().and_then(|p| p.to_str()) {
            Some(dir) => format!(
                "cd {} && claude {}\n",
                shell_single_quote(dir),
                shell_single_quote(claude_prompt),
            ),
            None => format!("claude {}\n", shell_single_quote(claude_prompt)),
        };

        if copy_to_clipboard(&command) {
            self.status_message = Some(format!(
                "appended {} to COMMENTS.md; claude command copied — paste in a new terminal",
                file_path
            ));
        } else {
            self.status_message = Some(format!(
                "appended {} to COMMENTS.md (clipboard copy failed)",
                file_path
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
}
