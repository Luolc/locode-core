//! The session picker (ADR-0029): choose a session to resume from a list.
//!
//! Split the way the rest of the UI is — a sans-IO [`PickerState`] that answers
//! keys, and a [`render`] that draws it — so the keys are table-testable and the
//! layout is snapshot-testable without a terminal.
//!
//! Shape is the one all three studied harnesses converged on: a title line plus a
//! dim metadata line per row, newest activity first, the current directory by
//! default with a key to widen (Claude Code `LogSelector.tsx:671-679`; codex
//! `tui/src/resume_picker.rs`; grok `app/app_view.rs:302-320`).

use std::path::{Path, PathBuf};

use locode_core::SessionSummary;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Rows a session occupies: the title, then its metadata.
const ROW_HEIGHT: usize = 2;

/// What the picker did with a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    /// Still open — repaint.
    Open,
    /// The user chose this rollout.
    Chosen(PathBuf),
    /// The user backed out.
    Cancelled,
}

/// The picker's whole state.
pub struct PickerState {
    /// Every session in scope, newest first (the host's order, preserved).
    all: Vec<SessionSummary>,
    /// Indices into `all` that survive the current filter, in the same order.
    shown: Vec<usize>,
    /// Cursor position **within `shown`**.
    cursor: usize,
    /// The substring filter, empty when off.
    filter: String,
    /// Whether the filter is taking keystrokes.
    filtering: bool,
    /// Titles, filled in only for rows about to be drawn.
    titles: Vec<Option<String>>,
    /// True once the scope has been widened past the starting directory.
    all_dirs: bool,
}

impl PickerState {
    /// Build over a listing (already sorted by the host).
    #[must_use]
    pub fn new(sessions: Vec<SessionSummary>, all_dirs: bool) -> Self {
        let shown = (0..sessions.len()).collect();
        let titles = vec![None; sessions.len()];
        Self {
            all: sessions,
            shown,
            cursor: 0,
            filter: String::new(),
            filtering: false,
            titles,
            all_dirs,
        }
    }

    /// Nothing to choose from — the caller says so instead of drawing an empty box.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Whether the scope has been widened to every directory.
    #[must_use]
    pub fn all_dirs(&self) -> bool {
        self.all_dirs
    }

    /// The highlighted session, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&SessionSummary> {
        self.shown.get(self.cursor).map(|&i| &self.all[i])
    }

    /// How many rows survive the filter.
    #[must_use]
    pub fn visible_count(&self) -> usize {
        self.shown.len()
    }

    /// Cursor position within the filtered rows (0-based).
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the filter currently owns keystrokes — the caller checks this
    /// before treating a printable key as a shortcut of its own.
    #[must_use]
    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    /// Feed a key. Returns what the caller should do next.
    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) -> PickerOutcome {
        use crossterm::event::KeyCode as K;
        // While the filter has focus, printable keys type into it — otherwise `a`
        // would silently mean "widen scope" in the middle of a search term.
        if self.filtering {
            match key.code {
                K::Esc => {
                    self.filtering = false;
                    self.filter.clear();
                    self.refilter();
                }
                K::Enter => self.filtering = false,
                K::Backspace => {
                    self.filter.pop();
                    self.refilter();
                }
                K::Char(c) => {
                    self.filter.push(c);
                    self.refilter();
                }
                K::Up => self.move_by(-1),
                K::Down => self.move_by(1),
                _ => {}
            }
            return PickerOutcome::Open;
        }
        match key.code {
            K::Esc => return PickerOutcome::Cancelled,
            K::Enter => {
                if let Some(session) = self.selected() {
                    return PickerOutcome::Chosen(session.path.clone());
                }
            }
            K::Up => self.move_by(-1),
            K::Down => self.move_by(1),
            K::Char('/') => self.filtering = true,
            _ => {}
        }
        PickerOutcome::Open
    }

    /// Replace the listing after a scope change, keeping the filter.
    pub fn replace(&mut self, sessions: Vec<SessionSummary>, all_dirs: bool) {
        self.titles = vec![None; sessions.len()];
        self.all = sessions;
        self.all_dirs = all_dirs;
        self.cursor = 0;
        self.refilter();
    }

    fn move_by(&mut self, delta: isize) {
        if self.shown.is_empty() {
            return;
        }
        let last = self.shown.len() - 1;
        // Clamp rather than wrap: wrapping in a long list moves the eye a whole
        // screen for one keypress.
        self.cursor = match delta {
            d if d < 0 => self.cursor.saturating_sub(d.unsigned_abs()),
            d => (self.cursor + d.unsigned_abs()).min(last),
        };
    }

    fn refilter(&mut self) {
        let needle = self.filter.to_lowercase();
        self.shown = (0..self.all.len())
            .filter(|&i| needle.is_empty() || self.haystack(i).contains(&needle))
            .collect();
        self.cursor = self.cursor.min(self.shown.len().saturating_sub(1));
    }

    /// What the filter matches against: everything the row displays, so a user can
    /// type what they see (Claude Code builds its search text the same way,
    /// `LogSelector.tsx:1540`).
    fn haystack(&self, i: usize) -> String {
        let s = &self.all[i];
        let title = self.titles[i].clone().unwrap_or_default();
        format!(
            "{} {} {} {} {}",
            title,
            s.id,
            s.harness,
            s.branch.clone().unwrap_or_default(),
            s.cwd.display()
        )
        .to_lowercase()
    }

    /// Read titles for the rows `render` is about to draw, and only those.
    ///
    /// The whole point of the two-pass design: a first paint that waited for every
    /// session's transcript would be slower than the `-c` it replaces.
    fn load_visible_titles(&mut self, first: usize, count: usize) {
        for &i in self.shown.iter().skip(first).take(count) {
            if self.titles[i].is_none() {
                self.titles[i] = Some(
                    locode_core::read_session_title(&self.all[i].path)
                        .unwrap_or_else(|| "(no prompt recorded)".to_string()),
                );
            }
        }
    }
}

/// Draw the picker into `area`.
pub fn render(frame: &mut crate::frame_terminal::Frame<'_>, area: Rect, state: &mut PickerState) {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines: Vec<Line<'static>> = Vec::new();

    if state.is_empty() {
        lines.push(Line::from(Span::styled(
            "Resume session",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            if state.all_dirs() {
                "No sessions recorded yet.".to_string()
            } else {
                "No sessions in this directory — press a to look in all directories.".to_string()
            },
            dim,
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "a all directories · esc cancel",
            dim,
        )));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }

    // Chrome: header, blank, then the footer hint and its blank.
    let chrome = 4usize;
    let rows = (area.height as usize).saturating_sub(chrome) / ROW_HEIGHT;
    let rows = rows.max(1);
    // Keep the cursor inside the window without recentering on every move.
    let first = state.cursor.saturating_sub(rows.saturating_sub(1));
    state.load_visible_titles(first, rows);

    let scope = if state.all_dirs() {
        "all directories"
    } else {
        "this directory"
    };
    lines.push(Line::from(vec![
        Span::styled(
            "Resume session",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  ({} of {} · {scope})",
                state.cursor + 1,
                state.visible_count()
            ),
            dim,
        ),
    ]));
    lines.push(Line::from(""));

    let width = area.width as usize;
    for (offset, &i) in state.shown.iter().skip(first).take(rows).enumerate() {
        let session = &state.all[i];
        let selected = first + offset == state.cursor;
        let marker = if selected { "❯ " } else { "  " };
        let title = state.titles[i].as_deref().unwrap_or("…");
        lines.push(Line::from(vec![
            Span::raw(marker.to_string()),
            Span::styled(
                truncate(title, width.saturating_sub(4)),
                if selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("    {}", meta_line(session, state.all_dirs())),
            dim,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if state.filtering {
            format!("filter: {}  (↵ apply · esc clear)", state.filter)
        } else {
            "↑↓ move · ↵ resume · / filter · a all directories · esc cancel".to_string()
        },
        dim,
    )));
    frame.render_widget(Paragraph::new(lines), area);
}

/// The dim second line: when, how big a conversation, and where it came from.
fn meta_line(session: &SessionSummary, show_dir: bool) -> String {
    let mut parts = vec![relative_age(session.last_active)];
    parts.push(session.harness.clone());
    if let Some(branch) = &session.branch {
        parts.push(branch.clone());
    }
    if show_dir {
        parts.push(short_path(&session.cwd));
    }
    parts.join(" · ")
}

/// `2h ago`-style age. Minute precision below an hour: a picker is read, not
/// watched, so seconds would be noise.
fn relative_age(when: std::time::SystemTime) -> String {
    let Ok(elapsed) = when.elapsed() else {
        return "just now".to_string();
    };
    let mins = elapsed.as_secs() / 60;
    match mins {
        0 => "just now".to_string(),
        1 => "1 min ago".to_string(),
        m if m < 60 => format!("{m} mins ago"),
        m if m < 120 => "1 hour ago".to_string(),
        m if m < 60 * 24 => format!("{} hours ago", m / 60),
        m if m < 60 * 48 => "yesterday".to_string(),
        m => format!("{} days ago", m / (60 * 24)),
    }
}

/// The last two path components — enough to tell projects apart without spending
/// the row on a home directory prefix (grok's `repo_name`, `app_view.rs:317`).
fn short_path(path: &Path) -> String {
    let mut parts: Vec<String> = path
        .components()
        .rev()
        .take(2)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.reverse();
    parts.join("/")
}

/// Truncate to `width` display columns, marking the cut.
fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width.saturating_sub(1) {
            out.push('…');
            return out;
        }
        out.push(ch);
        used += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::frame_terminal::FrameTerminal;

    /// Render into a test terminal, the same way the composer and dropdown tests
    /// already do (`ui/dropdown.rs:387`).
    fn draw(state: &mut PickerState, width: u16, height: u16) -> Vec<String> {
        let mut terminal = FrameTerminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, Rect::new(0, 0, width, height), state))
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn session(id: &str, mins_ago: u64, branch: &str) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            path: PathBuf::from(format!("/sessions/rollout-2026-07-29T00-00-00-{id}.jsonl")),
            cwd: PathBuf::from("/home/dev/locode-core"),
            harness: "claude".to_string(),
            model: "test-model".to_string(),
            branch: Some(branch.to_string()),
            last_active: SystemTime::now() - Duration::from_mins(mins_ago),
        }
    }

    fn with_titles(sessions: Vec<SessionSummary>, titles: &[&str]) -> PickerState {
        let mut state = PickerState::new(sessions, false);
        state.titles = titles.iter().map(|t| Some((*t).to_string())).collect();
        state
    }

    /// Cursor movement clamps at both ends — wrapping in a long list moves the eye
    /// a whole screen for one keypress.
    #[test]
    fn the_cursor_clamps_instead_of_wrapping() {
        let mut state = with_titles(
            vec![session("a", 5, "main"), session("b", 60, "main")],
            &["first", "second"],
        );
        assert_eq!(state.cursor(), 0);
        state.on_key(key(KeyCode::Up));
        assert_eq!(state.cursor(), 0, "already at the top");
        state.on_key(key(KeyCode::Down));
        state.on_key(key(KeyCode::Down));
        assert_eq!(state.cursor(), 1, "clamped at the last row, not wrapped");
    }

    /// Enter returns the highlighted rollout's path; Esc backs out.
    #[test]
    fn enter_chooses_the_highlighted_session_and_esc_cancels() {
        let mut state = with_titles(
            vec![session("a", 5, "main"), session("b", 60, "main")],
            &["first", "second"],
        );
        state.on_key(key(KeyCode::Down));
        let chosen = state.on_key(key(KeyCode::Enter));
        assert!(
            matches!(chosen, PickerOutcome::Chosen(ref p) if p.to_string_lossy().contains("-b.jsonl")),
            "{chosen:?}"
        );
        assert_eq!(state.on_key(key(KeyCode::Esc)), PickerOutcome::Cancelled);
    }

    /// `/` hands keystrokes to the filter, so ordinary letters type instead of
    /// acting as shortcuts; Esc clears the filter without closing the picker.
    #[test]
    fn the_filter_owns_keystrokes_while_open() {
        let mut state = with_titles(
            vec![session("a", 5, "main"), session("b", 60, "feature")],
            &["fix the teardown", "add the picker"],
        );
        state.on_key(key(KeyCode::Char('/')));
        assert!(state.is_filtering(), "`a` must not widen scope now");
        for c in "picker".chars() {
            state.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(state.visible_count(), 1);
        assert_eq!(state.selected().map(|s| s.id.as_str()), Some("b"));

        // Esc inside the filter clears it and stays open — it does not cancel.
        assert_eq!(state.on_key(key(KeyCode::Esc)), PickerOutcome::Open);
        assert_eq!(state.visible_count(), 2);
        assert!(!state.is_filtering());
    }

    /// The filter matches what the row shows — branch and harness included, not
    /// just the title.
    #[test]
    fn the_filter_matches_visible_metadata_too() {
        let mut state = with_titles(
            vec![session("a", 5, "main"), session("b", 60, "hotfix")],
            &["one", "two"],
        );
        state.on_key(key(KeyCode::Char('/')));
        for c in "hotfix".chars() {
            state.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(state.selected().map(|s| s.id.as_str()), Some("b"));
    }

    /// The **row shape**: two lines per session, the cursor on the first, the dim
    /// metadata under it. Pinned as a whole-screen snapshot because the shape is
    /// the feature — a widget that renders "correctly" one field at a time can
    /// still read wrong as a list.
    #[test]
    fn the_rendered_rows_match_the_snapshot() {
        let mut state = with_titles(
            vec![session("aaa", 5, "main"), session("bbb", 200, "feature/x")],
            &["fix the kitty teardown on every exit path", "add --add-dir"],
        );
        assert_eq!(
            draw(&mut state, 64, 10),
            vec![
                "Resume session  (1 of 2 · this directory)".to_string(),
                String::new(),
                "❯ fix the kitty teardown on every exit path".to_string(),
                "    5 mins ago · claude · main".to_string(),
                "  add --add-dir".to_string(),
                "    3 hours ago · claude · feature/x".to_string(),
                String::new(),
                "↑↓ move · ↵ resume · / filter · a all directories · esc cancel".to_string(),
                String::new(),
                String::new(),
            ],
            "row shape changed — is that intended?"
        );
    }

    /// An empty list says so, and says how to widen — a blank box would read as a
    /// broken screen.
    #[test]
    fn an_empty_picker_explains_itself() {
        let mut state = PickerState::new(Vec::new(), false);
        let text = draw(&mut state, 72, 6).join("\n");
        assert!(text.contains("No sessions in this directory"), "{text}");
        assert!(
            text.contains("press a to look in all directories"),
            "{text}"
        );
    }
}
