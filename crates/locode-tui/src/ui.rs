//! Rendering: the live region (bottom-anchored) and its widgets.
//!
//! The live region is the ONLY repainted surface (SPEC-TUI rendering model);
//! finalized transcript blocks are printed once into native scrollback via
//! `insert_before`. Blank rows above the status row read as margin.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::{App, Hint, RunState};

pub mod blocks;
pub mod composer;
pub mod highlight;
pub mod markdown;

/// Braille spinner frames (the four-harness standard).
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Draw the live region: flexible blank space, then status row (while
/// running), then the composer OR the approval overlay, then footer.
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    // The approval overlay replaces the composer while a decision is pending
    // (grok's front-only render).
    if let Some(view) = app.approval_queue.front() {
        let lines = approval_lines(view);
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let [_, overlay_area, footer_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(Paragraph::new(lines), overlay_area);
        frame.render_widget(Paragraph::new(approval_footer()), footer_area);
        return;
    }

    let composer_height = app.composer.desired_height(frame.area().width);
    let status_height = u16::from(app.is_running());
    let queue_height = u16::try_from(app.prompt_queue.len()).unwrap_or(u16::MAX);
    let [_, status_area, queue_area, composer_area, footer_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(status_height),
        Constraint::Length(queue_height),
        Constraint::Length(composer_height),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    if app.is_running() {
        frame.render_widget(Paragraph::new(status_line(app)), status_area);
    }
    if !app.prompt_queue.is_empty() {
        frame.render_widget(Paragraph::new(queue_lines(app)), queue_area);
    }
    app.composer.render(frame, composer_area);
    frame.render_widget(Paragraph::new(footer_line(app)), footer_area);
}

/// Dim `queued: …` previews for prompts waiting to run.
fn queue_lines(app: &App) -> Vec<Line<'static>> {
    app.prompt_queue
        .iter()
        .map(|text| {
            let one_line = text.replace('\n', " ");
            Line::styled(
                format!("queued: {one_line}"),
                Style::default().add_modifier(Modifier::DIM),
            )
        })
        .collect()
}

/// The approval overlay body: `⚠ Allow <tool>?` + dimmed args.
fn approval_lines(view: &crate::approval::ApprovalView) -> Vec<Line<'static>> {
    use ratatui::style::Color;
    use ratatui::text::Span;
    let title = Line::from(vec![
        Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("Allow {}?", view.tool_name),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);
    let args = Line::styled(
        format!("  {}", view.args),
        Style::default().add_modifier(Modifier::DIM),
    );
    vec![title, args]
}

fn approval_footer() -> Line<'static> {
    Line::styled(
        "y allow · a allow for session · d/esc deny",
        Style::default().add_modifier(Modifier::DIM),
    )
}

/// The single-row turn status (grok's shape at v1 scale:
/// `⠧ run_terminal_cmd · 12s`).
fn status_line(app: &App) -> Line<'static> {
    let RunState::Running {
        started,
        cancelling,
    } = app.run
    else {
        return Line::from("");
    };
    let spinner = SPINNER[(app.spinner_frame / 2) % SPINNER.len()];
    let activity = if cancelling {
        "cancelling…".to_string()
    } else {
        app.pending_tools
            .last()
            .map_or_else(|| "thinking".to_string(), |t| t.name.clone())
    };
    let elapsed = started.elapsed().as_secs();
    Line::styled(
        format!("{spinner} {activity} · {elapsed}s"),
        Style::default().add_modifier(Modifier::BOLD),
    )
}

/// The bottom status line: `cwd · model · N tok` when idle (user choice,
/// 2026-07-22), replaced by the transient armed-key hints when one is active.
/// Git branch and cost/usage-with-cap are deferred (named extension points).
fn footer_line(app: &App) -> Line<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let text = match app.hint {
        Some(Hint::QuitArmed) => "press ctrl+c again to quit".to_string(),
        Some(Hint::ClearArmed) => "press esc again to clear".to_string(),
        Some(Hint::Cancelling) => "cancelling — esc again to retry".to_string(),
        None => status_text(app),
    };
    Line::styled(text, dim)
}

/// Assemble the idle status text from whatever fields are known.
fn status_text(app: &App) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cwd) = &app.cwd {
        parts.push(cwd.clone());
    }
    if let Some(model) = &app.model {
        parts.push(model.clone());
    }
    if app.session_tokens > 0 {
        parts.push(format!("{} tok", fmt_tokens(app.session_tokens)));
    }
    parts.join(" · ")
}

/// Compact token count: `842`, `3.1k`, `1.2M`.
// Token counts are small enough that f64 precision loss is irrelevant here.
#[allow(clippy::cast_precision_loss)]
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PendingTool;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::Instant;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn draw_renders_composer_bottom_anchored_with_status() {
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut app = App::new();
        app.cwd = Some("~/dev/locode-core".into());
        app.model = Some("claude-sonnet-5".into());
        app.composer.insert_text("hello tui");

        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = buffer_text(&terminal);

        assert!(text.contains("❯ hello tui"), "typed text visible: {text}");
        assert!(
            text.contains("~/dev/locode-core"),
            "status line shows cwd on the last row: {text}"
        );
        let first_row: String = text.lines().next().unwrap().trim().to_string();
        assert!(first_row.is_empty(), "top row is margin: {first_row:?}");
    }

    #[test]
    fn status_line_shows_cwd_model_and_tokens() {
        let mut app = App::new();
        app.cwd = Some("~/proj".into());
        app.model = Some("opus".into());
        app.session_tokens = 3100;
        assert_eq!(footer_line(&app).to_string(), "~/proj · opus · 3.1k tok");
    }

    #[test]
    fn status_row_shows_spinner_and_active_tool_while_running() {
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
        let mut app = App::new();
        app.run = RunState::Running {
            started: Instant::now(),
            cancelling: false,
        };
        app.pending_tools.push(PendingTool {
            id: "c1".into(),
            name: "run_terminal_cmd".into(),
            args: "{}".into(),
        });

        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("run_terminal_cmd ·"), "{text}");

        // Idle: no status row.
        app.run = RunState::Idle;
        app.pending_tools.clear();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(!text.contains("thinking"), "{text}");
    }

    #[test]
    fn footer_swaps_to_armed_hints() {
        let mut app = App::new();
        app.hint = Some(Hint::QuitArmed);
        let line = footer_line(&app);
        assert!(line.to_string().contains("again to quit"));
    }
}
