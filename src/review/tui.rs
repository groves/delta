use std::io::{self, stdout};

use anyhow::{Context, Result};
use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::app::App;

pub fn run_tui(app: &mut App) -> Result<()> {
    enable_raw_mode().context("Failed to enable raw mode")?;
    stdout()
        .execute(EnterAlternateScreen)
        .context("Failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    let result = run_event_loop(&mut terminal, app);

    disable_raw_mode().context("Failed to disable raw mode")?;
    stdout()
        .execute(LeaveAlternateScreen)
        .context("Failed to leave alternate screen")?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    if app.hunks.is_empty() {
        return Ok(());
    }

    loop {
        terminal.draw(|frame| draw(frame, app))?;

        if let Event::Key(key) = event::read().context("Failed to read event")? {
            if key.code == KeyCode::Char('e')
                || key.code == KeyCode::Char('c')
                || key.code == KeyCode::Char('S')
                || key.code == KeyCode::Char('a')
            {
                // Leave TUI, run editor/comment/submit/ask-claude, then restore and force full redraw.
                disable_raw_mode().ok();
                stdout().execute(LeaveAlternateScreen).ok();
                match key.code {
                    KeyCode::Char('c') => {
                        app.start_comment();
                    }
                    KeyCode::Char('S') => {
                        app.submit_review();
                    }
                    KeyCode::Char('a') => {
                        app.ask_claude();
                    }
                    _ => {
                        app.open_in_editor();
                    }
                }
                enable_raw_mode().ok();
                stdout().execute(EnterAlternateScreen).ok();
                terminal.clear()?;
            } else {
                handle_key(key, app);
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Handle a single key event, updating app state. Extracted so tests can drive
/// the app without a real terminal.
fn handle_key(key: KeyEvent, app: &mut App) {
    // Any key press dismisses a transient status message.
    app.status_message = None;

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            if app.show_help {
                app.show_help = false;
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('?') => app.show_help = !app.show_help,
        KeyCode::Char('j') | KeyCode::Down => {
            app.scroll_down(1);
            app.update_current_hunk_from_scroll();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll_up(1);
            app.update_current_hunk_from_scroll();
        }
        KeyCode::Char('d') => {
            app.scroll_down(20);
            app.update_current_hunk_from_scroll();
        }
        KeyCode::Char('u') => {
            app.scroll_up(20);
            app.update_current_hunk_from_scroll();
        }
        KeyCode::Char('n') | KeyCode::Char(']') => app.next_hunk(),
        KeyCode::Char('p') | KeyCode::Char('[') => app.prev_hunk(),
        KeyCode::Char(' ') => app.toggle_viewed(),
        KeyCode::Char('g') => app.open_in_github(),
        _ => {}
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // diff content
            Constraint::Length(2), // status + keybindings
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);
    draw_diff(frame, app, chunks[1]);
    draw_status_bar(frame, app, chunks[2]);

    if app.show_help {
        draw_help_overlay(frame, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let total = app.hunks.len();
    let current = if total > 0 { app.current_hunk + 1 } else { 0 };
    let viewed = app.viewed_count();

    let title = if app.pr_metadata.number > 0 {
        format!(
            " PR #{}: {}  ({}/{} hunks, {} viewed)",
            app.pr_metadata.number, app.pr_metadata.title, current, total, viewed,
        )
    } else {
        format!(
            " {}  ({}/{} hunks, {} viewed)",
            app.pr_metadata.title, current, total, viewed,
        )
    };

    let header = Paragraph::new(title).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    frame.render_widget(header, area);
}

fn draw_diff(frame: &mut Frame, app: &App, area: Rect) {
    // Build a single concatenated Text from all hunks with separators between them.
    let mut lines: Vec<Line<'_>> = Vec::new();

    for (i, hunk) in app.hunks.iter().enumerate() {
        // Add separator line before each hunk (except the first).
        if i > 0 {
            let is_selected = i == app.current_hunk;
            let sep_style = if is_selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(
                "─".repeat(area.width as usize),
                sep_style,
            )));
        }

        let is_viewed = app.viewed.contains(&hunk.content_hash);
        if is_viewed {
            // Collapsed: single summary line for viewed hunks.
            let is_selected = i == app.current_hunk;
            let style = if is_selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(
                format!("  [viewed] {}:{}", hunk.file_path, hunk.plus_start),
                style,
            )));
        } else {
            // Expanded: full rendered diff.
            for line in &hunk.rendered.lines {
                lines.push(line.clone());
            }
        }
    }

    let text = Text::from(lines);
    let paragraph = Paragraph::new(text)
        .scroll((app.scroll_offset, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let status_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    let primary_text = if let Some(msg) = &app.status_message {
        format!(" {}", msg)
    } else if let Some(hunk) = app.current_hunk() {
        let viewed_marker = if app.is_current_viewed() {
            " [viewed]"
        } else {
            ""
        };
        format!(" {}:{}{}", hunk.file_path, hunk.plus_start, viewed_marker)
    } else {
        " No hunks".to_string()
    };

    let status =
        Paragraph::new(primary_text).style(Style::default().fg(Color::White).bg(Color::DarkGray));
    frame.render_widget(status, status_chunks[0]);

    let hints_text = if app.pr_metadata.repo.is_empty() {
        " n/p:hunk  j/k:scroll  space:viewed  e:editor  a:claude  ?:help  q:quit".to_string()
    } else if app.pending_comments.is_empty() {
        " n/p:hunk  j/k:scroll  space:viewed  e:editor  c:comment  a:claude  g:github  ?:help  q:quit"
            .to_string()
    } else {
        format!(
            " n/p:hunk  j/k:scroll  space:viewed  e:editor  c:comment  a:claude  S:submit({})  g:github  ?:help  q:quit",
            app.pending_comments.len()
        )
    };
    let hints = Paragraph::new(hints_text.as_str()).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hints, status_chunks[1]);
}

fn draw_help_overlay(frame: &mut Frame, area: Rect) {
    let help_width = 50.min(area.width.saturating_sub(4));
    let help_height = 17.min(area.height.saturating_sub(4));

    let help_area = Rect {
        x: (area.width - help_width) / 2,
        y: (area.height - help_height) / 2,
        width: help_width,
        height: help_height,
    };

    frame.render_widget(Clear, help_area);

    let help_text = vec![
        Line::from("Keybindings").style(Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::from("j / Down     Scroll down"),
        Line::from("k / Up       Scroll up"),
        Line::from("d            Scroll down half page"),
        Line::from("u            Scroll up half page"),
        Line::from("n / ]        Next hunk"),
        Line::from("p / [        Previous hunk"),
        Line::from("Space        Toggle viewed"),
        Line::from("e            Open in $EDITOR"),
        Line::from("c            Comment on hunk (PR only)"),
        Line::from("S            Submit review (PR only)"),
        Line::from("a            Append to COMMENTS.md + copy claude cmd"),
        Line::from("g            Open in GitHub"),
        Line::from("q / Esc      Quit"),
    ];

    let help = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).title(" Help "))
        .style(Style::default().fg(Color::White).bg(Color::Black));

    frame.render_widget(help, help_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::app::{App, ReviewHunk};
    use crate::review::github::PrMetadata;
    use ratatui::backend::TestBackend;
    use ratatui::text::{Line, Text};
    use std::collections::HashSet;

    fn make_hunk(path: &str, hash: &str, num_lines: usize) -> ReviewHunk {
        let lines: Vec<Line<'static>> = (0..num_lines)
            .map(|i| Line::from(format!("{hash} line {i}")))
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
            number: 42,
            title: "Test PR".to_string(),
            repo: "test/repo".to_string(),
            head_sha: String::new(),
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Render the app into a TestBackend buffer and return the buffer content
    /// as a vector of strings (one per row).
    fn render(app: &App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut rows = Vec::new();
        for y in 0..height {
            let mut row = String::new();
            for x in 0..width {
                row.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            rows.push(row);
        }
        rows
    }

    /// Helper: check if any row in the rendered output contains the given substring.
    fn screen_contains(rows: &[String], needle: &str) -> bool {
        rows.iter().any(|row| row.contains(needle))
    }

    #[test]
    fn initial_render_shows_first_hunk_content() {
        let hunks = vec![make_hunk("a.rs", "h1", 3), make_hunk("b.rs", "h2", 3)];
        let app = App::new(hunks, HashSet::new(), make_metadata());
        let rows = render(&app, 80, 20);

        // Header should show PR info.
        assert!(screen_contains(&rows, "PR #42"));
        assert!(screen_contains(&rows, "1/2 hunks"));
        // First hunk content should be visible.
        assert!(screen_contains(&rows, "h1 line 0"));
        // Status bar should show the file path.
        assert!(screen_contains(&rows, "a.rs:1"));
    }

    #[test]
    fn pressing_v_collapses_hunk_and_advances() {
        let hunks = vec![make_hunk("a.rs", "h1", 5), make_hunk("b.rs", "h2", 5)];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());

        // Initial: hunk 0 is current, its content is visible.
        let rows = render(&app, 80, 20);
        assert!(screen_contains(&rows, "h1 line 0"));

        // Press 'v' to mark hunk 0 as viewed.
        handle_key(press(KeyCode::Char(' ')), &mut app);

        let rows = render(&app, 80, 20);
        // Should have advanced to hunk 1.
        assert!(screen_contains(&rows, "h2 line 0"));
        // Hunk 0 expanded content should NOT appear.
        assert!(
            !screen_contains(&rows, "h1 line 0"),
            "viewed hunk content should not appear expanded"
        );
        // Header should show 2/2 hunks (advanced) and 1 viewed.
        assert!(screen_contains(&rows, "2/2 hunks, 1 viewed"));

        // Navigate back to hunk 0 to verify it's collapsed.
        handle_key(press(KeyCode::Char('p')), &mut app);

        let rows = render(&app, 80, 20);
        assert!(
            screen_contains(&rows, "[viewed] a.rs:1"),
            "navigating back to viewed hunk should show collapsed summary"
        );
        // Expanded content should still not appear.
        assert!(!screen_contains(&rows, "h1 line 0"));
    }

    #[test]
    fn pressing_v_on_viewed_hunk_expands_it() {
        let hunks = vec![make_hunk("a.rs", "h1", 5), make_hunk("b.rs", "h2", 5)];
        let viewed: HashSet<String> = ["h1".to_string()].into();
        let mut app = App::new(hunks, viewed, make_metadata());

        // App should start at hunk 1 (first unviewed).
        assert_eq!(app.current_hunk, 1);

        // Navigate back to hunk 0.
        handle_key(press(KeyCode::Char('p')), &mut app);
        assert_eq!(app.current_hunk, 0);

        // Hunk 0 is collapsed.
        let rows = render(&app, 80, 20);
        assert!(screen_contains(&rows, "[viewed] a.rs:1"));
        assert!(!screen_contains(&rows, "h1 line 0"));

        // Press 'v' to unview — should expand.
        handle_key(press(KeyCode::Char(' ')), &mut app);

        let rows = render(&app, 80, 20);
        assert!(
            screen_contains(&rows, "h1 line 0"),
            "unviewed hunk should be expanded"
        );
        assert!(!screen_contains(&rows, "[viewed] a.rs:1"));
        // Should stay on hunk 0.
        assert_eq!(app.current_hunk, 0);
    }

    #[test]
    fn starts_at_first_unviewed_hunk() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 3),
            make_hunk("b.rs", "h2", 3),
            make_hunk("c.rs", "h3", 3),
        ];
        let viewed: HashSet<String> = ["h1".to_string(), "h2".to_string()].into();
        let app = App::new(hunks, viewed, make_metadata());

        let rows = render(&app, 80, 20);
        // Should show hunk 3 (first unviewed) content.
        assert!(screen_contains(&rows, "h3 line 0"));
        // Status bar should show c.rs.
        assert!(screen_contains(&rows, "c.rs:1"));
        // Header: 3/3 hunks.
        assert!(screen_contains(&rows, "3/3 hunks, 2 viewed"));
    }

    #[test]
    fn navigation_through_collapsed_hunks() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 3),
            make_hunk("b.rs", "h2", 3),
            make_hunk("c.rs", "h3", 3),
        ];
        let viewed: HashSet<String> = ["h2".to_string()].into();
        let mut app = App::new(hunks, viewed, make_metadata());
        // Starts at hunk 0 (first unviewed).
        assert_eq!(app.current_hunk, 0);

        // Navigate forward: n → hunk 1 (collapsed), n → hunk 2.
        handle_key(press(KeyCode::Char('n')), &mut app);
        assert_eq!(app.current_hunk, 1);
        let rows = render(&app, 80, 20);
        assert!(
            screen_contains(&rows, "[viewed] b.rs:1"),
            "navigating to viewed hunk should show collapsed summary"
        );
        assert!(screen_contains(&rows, "b.rs:1 [viewed]")); // status bar

        handle_key(press(KeyCode::Char('n')), &mut app);
        assert_eq!(app.current_hunk, 2);
        let rows = render(&app, 80, 20);
        assert!(screen_contains(&rows, "h3 line 0"));
    }

    #[test]
    fn view_all_hunks_sequentially() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 3),
            make_hunk("b.rs", "h2", 3),
            make_hunk("c.rs", "h3", 3),
        ];
        let mut app = App::new(hunks, HashSet::new(), make_metadata());

        // View all three hunks.
        handle_key(press(KeyCode::Char(' ')), &mut app); // view h1, advance to h2
        assert_eq!(app.current_hunk, 1);
        handle_key(press(KeyCode::Char(' ')), &mut app); // view h2, advance to h3
        assert_eq!(app.current_hunk, 2);
        handle_key(press(KeyCode::Char(' ')), &mut app); // view h3, stay on h3 (last)
        assert_eq!(app.current_hunk, 2);

        assert_eq!(app.viewed_count(), 3);

        // Navigate to the first hunk to see all collapsed summaries.
        handle_key(press(KeyCode::Char('p')), &mut app);
        handle_key(press(KeyCode::Char('p')), &mut app);
        assert_eq!(app.current_hunk, 0);

        let rows = render(&app, 80, 20);
        // All hunks collapsed.
        assert!(screen_contains(&rows, "[viewed] a.rs:1"));
        assert!(screen_contains(&rows, "[viewed] b.rs:1"));
        assert!(screen_contains(&rows, "[viewed] c.rs:1"));
        // No expanded content.
        assert!(!screen_contains(&rows, "h1 line"));
        assert!(!screen_contains(&rows, "h2 line"));
        assert!(!screen_contains(&rows, "h3 line"));
        // Header: 3 viewed.
        assert!(screen_contains(&rows, "3 viewed"));
    }

    #[test]
    fn view_skips_over_already_viewed_hunks() {
        let hunks = vec![
            make_hunk("a.rs", "h1", 3),
            make_hunk("b.rs", "h2", 3),
            make_hunk("c.rs", "h3", 3),
            make_hunk("d.rs", "h4", 3),
        ];
        // h2 and h3 already viewed.
        let viewed: HashSet<String> = ["h2".to_string(), "h3".to_string()].into();
        let mut app = App::new(hunks, viewed, make_metadata());
        assert_eq!(app.current_hunk, 0);

        // Press 'v' on h1 — should skip collapsed h2, h3 and land on h4.
        handle_key(press(KeyCode::Char(' ')), &mut app);
        assert_eq!(app.current_hunk, 3);

        let rows = render(&app, 80, 20);
        assert!(screen_contains(&rows, "h4 line 0"));
        assert!(screen_contains(&rows, "d.rs:1"));
        assert!(screen_contains(&rows, "3 viewed"));
    }
}
