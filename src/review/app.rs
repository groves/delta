use std::collections::HashSet;

use ratatui::text::Text;

use super::github::PrMetadata;

pub struct ReviewHunk {
    pub file_path: String,
    pub content_hash: String,
    pub plus_start: usize,
    pub rendered: Text<'static>,
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
        };
        // Auto-mark lock files as viewed.
        for hunk in &app.hunks {
            if hunk.file_path.ends_with(".lock") {
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
            let file = &hunk.file_path;
            let line = hunk.plus_start;

            let _ = std::process::Command::new(&editor)
                .arg(format!("+{}", line))
                .arg(file)
                .status();
        }
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

    pub fn viewed_count(&self) -> usize {
        self.hunks
            .iter()
            .filter(|h| self.viewed.contains(&h.content_hash))
            .count()
    }
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
        }
    }

    fn make_metadata() -> PrMetadata {
        PrMetadata {
            number: 1,
            title: "test".to_string(),
            repo: "test/repo".to_string(),
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
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
        ];
        // h1 is viewed (collapsed = 1 line), h2 is not (10 lines).
        let viewed: HashSet<String> = ["h1".to_string()].into();
        let app = App::new(hunks, viewed, make_metadata());
        // hunk 0: offset=0, height=1 (collapsed), separator=1 → next at 2
        // hunk 1: offset=2
        assert_eq!(app.hunk_line_offsets, vec![0, 2]);
    }

    #[test]
    fn offsets_all_expanded() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
        ];
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
        assert_eq!(app.current_hunk, 3, "should skip viewed h2, h3 and land on h4");
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
        assert_eq!(app.current_hunk, 0, "should stay when no unviewed hunks remain after");
    }

    #[test]
    fn toggle_unview_expands_and_stays() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
        ];
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
        let hunks = vec![
            make_hunk("a.rs", "h1", 10),
            make_hunk("b.rs", "h2", 10),
        ];
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
        assert!(lines[0].contains("[viewed]"), "first line should be collapsed summary");
        assert_eq!(lines[1], "separator");
    }
}
