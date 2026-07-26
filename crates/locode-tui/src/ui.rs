//! Rendering: the bottom-anchored full-screen frame and its widgets (ADR-0022).
//!
//! Each paint is one relative frame — `[transcript tail] [status] [queue]
//! [composer] [footer]` — pinned to the screen bottom. Finalized transcript
//! that overflows the screen top is committed to native scrollback by the paint
//! loop's `scroll_up` (no `insert_before`). Blank rows above the tail read as
//! margin.

use chrono::{DateTime, Local};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Hint, RunState};

pub mod blocks;
pub mod composer;
pub mod dropdown;
pub mod highlight;
pub mod markdown;

/// Braille spinner frames (the four-harness standard).
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Draw one bottom-anchored full-screen frame (ADR-0022): a flexible top
/// margin, the recent transcript `tail`, the status row (while running), the
/// queued-prompt previews, the composer OR the approval overlay, then footer.
/// The caller supplies the already-clipped `tail` (rows that fit on screen; the
/// overflow has been committed to native scrollback) and `composer_rows` (the
/// screen-capped composer height from [`live_rows`]).
pub fn draw(
    frame: &mut crate::frame_terminal::Frame<'_>,
    app: &App,
    tail: &[Line<'static>],
    composer_rows: u16,
) {
    let tail_len = u16::try_from(tail.len()).unwrap_or(u16::MAX);

    // The approval overlay replaces the composer while a decision is pending
    // (grok's front-only render).
    if let Some(view) = app.approval_queue.front() {
        let lines = approval_lines(view);
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let [_, tail_area, overlay_area, footer_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(tail_len),
            Constraint::Length(height),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        frame.render_widget(Paragraph::new(tail.to_vec()), tail_area);
        frame.render_widget(Paragraph::new(lines), overlay_area);
        frame.render_widget(Paragraph::new(approval_footer()), footer_area);
        return;
    }

    // The live streaming cell (ADR-0021): the in-progress assistant text, pinned
    // just above the status row and redrawn each frame (never committed to
    // scrollback while live — it finalizes to an `AssistantText` block on turn end).
    let stream = streaming_cell(app, frame.area().width, frame.area().height, composer_rows);
    let stream_height = u16::try_from(stream.len()).unwrap_or(u16::MAX);
    let status_height = u16::from(app.is_running());
    let queue_height = u16::try_from(app.prompt_queue.len()).unwrap_or(u16::MAX);
    // The command menu sits between the queue and the composer — directly above the
    // text it is completing, so the eye travels from the row to the caret.
    let menu_height = dropdown::desired_rows(&app.slash, frame.area().width);
    let [
        _,
        tail_area,
        stream_area,
        status_area,
        queue_area,
        menu_area,
        composer_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(tail_len),
        Constraint::Length(stream_height),
        Constraint::Length(status_height),
        Constraint::Length(queue_height),
        Constraint::Length(menu_height),
        Constraint::Length(composer_rows),
        Constraint::Length(FOOTER_ROWS),
    ])
    .areas(frame.area());

    frame.render_widget(Paragraph::new(tail.to_vec()), tail_area);
    if !stream.is_empty() {
        frame.render_widget(Paragraph::new(stream), stream_area);
    }
    if app.is_running() {
        frame.render_widget(Paragraph::new(status_line(app)), status_area);
    }
    if !app.prompt_queue.is_empty() {
        frame.render_widget(Paragraph::new(queue_lines(app)), queue_area);
    }
    dropdown::render(frame, menu_area, &app.slash);
    app.composer
        .render_with_ghost(frame, composer_area, app.slash.ghost.as_deref());
    frame.render_widget(
        Paragraph::new(footer_lines(app, footer_area.width)),
        footer_area,
    );
}

/// The non-tail geometry for the next frame: the screen-capped composer height
/// and the total rows the frame occupies *below* the transcript tail (status +
/// queue + composer + footer, or the approval overlay + footer). The paint
/// loop uses `non_tail` to compute how many tail rows fit and how many overflow
/// into native scrollback.
#[must_use]
pub fn live_rows(app: &App, width: u16, screen_h: u16) -> (u16, u16) {
    if let Some(view) = app.approval_queue.front() {
        let overlay_len = u16::try_from(approval_lines(view).len()).unwrap_or(u16::MAX);
        // Overlay + footer; no composer while a decision is pending.
        return (0, overlay_len.saturating_add(1));
    }
    let composer = app
        .composer
        .desired_height(width)
        .min(crate::term::max_composer_rows(screen_h));
    let status = u16::from(app.is_running());
    let queue = u16::try_from(app.prompt_queue.len()).unwrap_or(u16::MAX);
    let stream =
        u16::try_from(streaming_cell(app, width, screen_h, composer).len()).unwrap_or(u16::MAX);
    let non_tail = status
        .saturating_add(queue)
        .saturating_add(stream)
        .saturating_add(dropdown::desired_rows(&app.slash, width))
        .saturating_add(composer)
        .saturating_add(FOOTER_ROWS);
    (composer, non_tail)
}

/// The live streaming cell's rendered rows (empty when not streaming), capped to
/// the height available in the live region — the screen minus everything pinned
/// below the cell (status + queue + composer + footer). The finalized tail above
/// takes what's left and commits its overflow to native scrollback, so a long
/// stream fills the screen showing its latest lines instead of a tiny window.
fn streaming_cell(app: &App, width: u16, screen_h: u16, composer_rows: u16) -> Vec<Line<'static>> {
    let Some(buffer) = app.streaming.as_deref() else {
        return Vec::new();
    };
    let status = u16::from(app.is_running());
    let queue = u16::try_from(app.prompt_queue.len()).unwrap_or(u16::MAX);
    let below = status
        .saturating_add(queue)
        .saturating_add(composer_rows)
        .saturating_add(FOOTER_ROWS);
    let max_rows = usize::from(screen_h.saturating_sub(below));
    // `first` places the message bullet only when no earlier block has committed.
    blocks::render_streaming(buffer, width, max_rows, !app.streaming_committed_any)
}

/// Dim `queued: …` previews for prompts waiting to run.
fn queue_lines(app: &App) -> Vec<Line<'static>> {
    app.prompt_queue
        .iter()
        .map(|queued| {
            let one_line = queued.display.replace('\n', " ");
            Line::styled(
                format!("  queued: {one_line}"),
                Style::default().add_modifier(Modifier::DIM),
            )
        })
        .collect()
}

/// The approval overlay body: `⚠ Allow <tool>?` + dimmed args.
fn approval_lines(view: &crate::approval::ApprovalView) -> Vec<Line<'static>> {
    let title = Line::from(vec![
        Span::raw("  "),
        Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("Allow {}?", view.tool_name),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);
    let args = Line::styled(
        format!("    {}", view.args),
        Style::default().add_modifier(Modifier::DIM),
    );
    vec![title, args]
}

fn approval_footer() -> Line<'static> {
    Line::styled(
        "    y allow · a allow for session · d/esc deny",
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
        format!("  {spinner} {activity} · {elapsed}s"),
        Style::default().add_modifier(Modifier::BOLD),
    )
}

/// Footer height in rows: a 2-row status bar with a component in each corner
/// (user layout, 2026-07-22).
const FOOTER_ROWS: u16 = 2;

/// 4-space left margin (aligns the status with the composer's input column,
/// 2 margin + `❯ `) and a 2-col right margin so corners don't hug the edge.
const FOOTER_LEFT_MARGIN: usize = 4;
const FOOTER_RIGHT_MARGIN: usize = 2;

/// Clock glyph before the time (Nerd Font `nf-fa-clock_o`), in the same dim gray
/// as the time. Requires a Nerd Font (the user's terminal has one).
const CLOCK_ICON: &str = "\u{f017}";

/// The bottom status bar: two rows, one component pinned in each corner (user
/// layout, 2026-07-22) —
/// ```text
///     <cwd>                      <clock> <time>
///     <model>                   <Context: N tokens>
/// ```
/// cwd is bright blue (matches the user's `ccstatusline` `current-working-dir`
/// color), model gray, tokens red ("Cayenne"), all **bold**; the time is dim
/// (the same lighter gray as the old separators) with a clock icon. A pending
/// armed-key hint replaces the cwd corner. Git branch and cost/usage-with-cap
/// are deferred (named extension points).
fn footer_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let bold = |color: Color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);

    // Row 1 left: armed hint (bold) or cwd (bright blue, bold).
    let left1 = if let Some(hint) = app.hint {
        let text = match hint {
            Hint::QuitArmed => "press ctrl+c again to quit",
            Hint::ClearArmed => "press esc again to clear",
            Hint::Cancelling => "cancelling — esc again to retry",
        };
        Some(Span::styled(
            text,
            Style::default().add_modifier(Modifier::BOLD),
        ))
    } else {
        app.cwd
            .as_ref()
            .map(|cwd| Span::styled(cwd.clone(), bold(Color::LightBlue)))
    };
    // Row 1 right: `<shell> · <clock icon> <local time>`, dim (same gray as the
    // old separators). The shell (what `run_terminal_cmd` uses) sits left of the
    // time, a `·` between; dropped until the engine reports it.
    let clock = format!("{CLOCK_ICON} {}", footer_clock(&Local::now()));
    let right1 = match app.shell.as_deref() {
        Some(shell) if !shell.is_empty() => format!("{shell} · {clock}"),
        _ => clock,
    };
    let time = Span::styled(right1, dim);

    // Row 2 left: model (gray, bold).
    let left2 = app
        .model
        .as_ref()
        .map(|model| Span::styled(model.clone(), bold(Color::Gray)));
    // Row 2 right: session tokens (red "Cayenne", bold) — always shown so the
    // corner stays populated (a 0-token fresh session still shows `0 tok`).
    // Labelled `Context:` — a bare count reads as "tokens used so far", which is a
    // different (and unbuilt) number; this one is how full the window is.
    let tokens = Span::styled(
        format!(
            "Context: {}{} tokens",
            if app.context_estimated { "~" } else { "" },
            fmt_tokens(app.context_tokens)
        ),
        bold(Color::Red),
    );

    vec![
        footer_row(left1, Some(time), width),
        footer_row(left2, Some(tokens), width),
    ]
}

/// Lay out one footer row: `left` pinned to the left margin, `right` pinned to
/// the right margin, blank fill between. When the row is too narrow to fit both
/// without touching, the right corner is dropped.
fn footer_row(
    left: Option<Span<'static>>,
    right: Option<Span<'static>>,
    width: u16,
) -> Line<'static> {
    let left_len = FOOTER_LEFT_MARGIN + left.as_ref().map_or(0, |s| s.content.chars().count());
    let total = usize::from(width);

    let mut spans = vec![Span::raw(" ".repeat(FOOTER_LEFT_MARGIN))];
    spans.extend(left);
    if let Some(right) = right {
        let right_len = right.content.chars().count();
        if total > left_len + right_len + FOOTER_RIGHT_MARGIN {
            let pad = total - left_len - right_len - FOOTER_RIGHT_MARGIN;
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(right);
        }
    }
    Line::from(spans)
}

/// Format a clock stamp for the footer: full local date + `HH:MM`, no timezone
/// label. `chrono::Local`'s `%Z` can only print the numeric offset (`-07:00`,
/// not `PDT`) — a real abbreviation would need a tz-database dep — so it's
/// dropped; the time itself still honors the `TZ` env var and `/etc/localtime`,
/// so `TZ=America/Los_Angeles locode` (or a shell that exports `TZ`) sets the
/// zone, matching a zsh status bar; no in-app timezone config needed. Minute
/// precision (not seconds) because the UI has zero idle repaints (`event_loop`:
/// ticks only while a run is active) — the clock refreshes on the next paint,
/// like a shell prompt, so ticking seconds would look frozen between keystrokes.
fn footer_clock<Tz: chrono::TimeZone>(now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    now.format("%Y-%m-%d %H:%M").to_string()
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
    use crate::frame_terminal::FrameTerminal;
    use ratatui::backend::TestBackend;
    use std::time::Instant;

    fn buffer_text(terminal: &FrameTerminal<TestBackend>) -> String {
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
        let mut terminal = FrameTerminal::new(TestBackend::new(40, 10)).unwrap();
        let mut app = App::new();
        app.cwd = Some("~/dev/locode-core".into());
        app.model = Some("claude-sonnet-5".into());
        app.composer.insert_text("hello tui");

        let (cr, _) = live_rows(&app, 40, 10);
        terminal.draw(|frame| draw(frame, &app, &[], cr)).unwrap();
        let text = buffer_text(&terminal);

        assert!(text.contains("❯ hello tui"), "typed text visible: {text}");
        assert!(
            text.contains("~/dev/locode-core"),
            "status line shows cwd on the last row: {text}"
        );
        let first_row: String = text.lines().next().unwrap().trim().to_string();
        assert!(first_row.is_empty(), "top row is margin: {first_row:?}");
    }

    /// The command menu takes rows directly above the composer, and gives them back
    /// when it closes.
    #[test]
    fn the_command_menu_sits_between_the_transcript_and_the_composer() {
        let mut terminal = FrameTerminal::new(TestBackend::new(50, 12)).unwrap();
        let mut app = App::new();
        app.model = Some("mock-1".into());
        for ch in "/".chars() {
            let _ = app.update(
                crate::app::Msg::Input(Box::new(crossterm::event::Event::Key(
                    crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(ch)),
                ))),
                Instant::now(),
            );
        }
        assert!(app.slash.open, "precondition: the menu is open");

        let (cr, non_tail) = live_rows(&app, 50, 12);
        terminal.draw(|frame| draw(frame, &app, &[], cr)).unwrap();
        let text = buffer_text(&terminal);
        let rows: Vec<&str> = text.lines().collect();
        // Any menu row will do — this asserts geometry, not contents, and the
        // command set is free to grow past what a 12-row frame can show.
        let menu_row = rows
            .iter()
            .position(|r| r.contains("/effort"))
            .expect("a menu row is on screen");
        // The selected menu row carries a `❯` of its own, so the composer is the
        // *last* one on screen.
        let composer_row = rows.iter().rposition(|r| r.contains('❯')).unwrap();
        assert!(menu_row < composer_row, "menu above the composer: {text}");
        assert!(
            non_tail >= cr + 2,
            "the menu is reserved in the frame geometry ({non_tail} vs composer {cr})"
        );
    }

    /// The fuzzy matcher's indices reach the screen: the letters the query matched are
    /// blue, and their neighbours are not.
    #[test]
    fn the_letters_a_query_matched_render_blue() {
        let mut terminal = FrameTerminal::new(TestBackend::new(50, 12)).unwrap();
        let mut app = App::new();
        app.model = Some("mock-1".into());
        // `qt` is a subsequence of `quit`, not a substring — this only matches at all
        // because the ranking is fuzzy.
        for ch in "/qt".chars() {
            let _ = app.update(
                crate::app::Msg::Input(Box::new(crossterm::event::Event::Key(
                    crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(ch)),
                ))),
                Instant::now(),
            );
        }
        assert_eq!(
            app.slash.matches.len(),
            1,
            "precondition: only /quit matches"
        );

        let (cr, _) = live_rows(&app, 50, 12);
        terminal.draw(|frame| draw(frame, &app, &[], cr)).unwrap();
        let buf = terminal.backend().buffer();
        let row = (0..12)
            .find(|&y| {
                (0..50)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains("/quit")
            })
            .expect("the menu row is on screen");

        // Label starts at column 4 (2 margin + 2 prefix): `/ q u i t`.
        let at = |x: u16| (buf[(x, row)].symbol().to_string(), buf[(x, row)].style().fg);
        let blue = Some(Color::LightBlue);
        assert_eq!(at(5), ("q".into(), blue), "matched");
        assert_eq!(at(8), ("t".into(), blue), "matched");
        assert_eq!(at(6).0, "u");
        assert_ne!(at(6).1, blue, "unmatched letter is not blue");
        assert_ne!(at(4).1, blue, "the slash is not blue");
    }

    #[test]
    fn footer_is_two_rows_cwd_time_over_model_tokens() {
        let mut app = App::new();
        app.cwd = Some("~/proj".into());
        app.model = Some("opus".into());
        app.shell = Some("zsh".into());
        app.context_tokens = 3100;
        let rows = footer_lines(&app, 80);
        assert_eq!(rows.len(), 2, "footer is a 2-row status bar");
        let (r0, r1) = (rows[0].to_string(), rows[1].to_string());
        // Row 1: cwd left; `<shell> · <clock> <time>` right.
        assert!(r0.starts_with("    ~/proj"), "cwd top-left: {r0:?}");
        assert!(r0.contains(CLOCK_ICON), "clock icon present: {r0:?}");
        assert!(
            r0.contains(&format!("zsh · {CLOCK_ICON}")),
            "shell sits left of the time, `·` between: {r0:?}"
        );
        assert!(
            r0.trim_end().ends_with(|c: char| c.is_ascii_digit()),
            "time ends the row: {r0:?}"
        );
        // Row 2: model left, tokens right.
        assert!(r1.starts_with("    opus"), "model bottom-left: {r1:?}");
        assert!(
            r1.trim_end().ends_with("Context: 3.1k tokens"),
            "tokens bottom-right: {r1:?}"
        );
    }

    #[test]
    fn footer_tokens_corner_shows_even_at_zero() {
        let app = App::new();
        // Fresh session (0 tokens) still populates the tokens corner.
        let rows = footer_lines(&app, 80);
        assert!(
            rows[1]
                .to_string()
                .trim_end()
                .ends_with("Context: 0 tokens"),
            "{:?}",
            rows[1].to_string()
        );
    }

    #[test]
    fn footer_clock_formats_full_local_date() {
        use chrono::{TimeZone, Utc};
        let dt = Utc.with_ymd_and_hms(2026, 7, 22, 0, 53, 7).unwrap();
        // Full date, minute precision (no seconds), no timezone label.
        assert_eq!(footer_clock(&dt), "2026-07-22 00:53");
    }

    #[test]
    fn footer_row_pins_corners_and_drops_right_when_narrow() {
        let left = Some(Span::raw("~/proj"));
        let right = Some(Span::raw("2026-07-22 00:53"));
        let line = footer_row(left.clone(), right.clone(), 40);
        let s = line.to_string();
        assert_eq!(
            s.chars().count(),
            40 - FOOTER_RIGHT_MARGIN,
            "right corner ends 2 cols from the edge"
        );
        assert!(s.starts_with("    ~/proj"), "left corner first: {s:?}");
        assert!(
            s.ends_with("2026-07-22 00:53"),
            "right corner pinned: {s:?}"
        );
        // Too narrow to fit both — only the left corner shows, no panic.
        let narrow = footer_row(left, right, 20);
        assert_eq!(narrow.to_string(), "    ~/proj");
    }

    #[test]
    fn footer_components_are_bold_in_their_colors() {
        let mut app = App::new();
        app.cwd = Some("~/proj".into());
        app.model = Some("opus".into());
        app.context_tokens = 3100;
        let rows = footer_lines(&app, 80);
        // Corner colors (user scheme, 2026-07-22): cwd bright blue, model gray,
        // tokens red ("Cayenne"), all bold; the time is dim, not bold.
        let cwd = &rows[0].spans[1];
        assert_eq!(cwd.content.as_ref(), "~/proj");
        assert_eq!(cwd.style.fg, Some(Color::LightBlue));
        assert!(cwd.style.add_modifier.contains(Modifier::BOLD));
        let time = rows[0].spans.last().unwrap();
        assert!(
            time.style.add_modifier.contains(Modifier::DIM),
            "time is dim"
        );
        assert!(
            !time.style.add_modifier.contains(Modifier::BOLD),
            "time not bold"
        );
        let model = &rows[1].spans[1];
        assert_eq!(model.style.fg, Some(Color::Gray), "model gray");
        let tokens = rows[1].spans.last().unwrap();
        assert_eq!(tokens.style.fg, Some(Color::Red), "tokens cayenne/red");
        assert!(tokens.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn status_row_shows_spinner_and_active_tool_while_running() {
        let mut terminal = FrameTerminal::new(TestBackend::new(50, 10)).unwrap();
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

        let (cr, _) = live_rows(&app, 50, 10);
        terminal.draw(|frame| draw(frame, &app, &[], cr)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("run_terminal_cmd ·"), "{text}");

        // Idle: no status row.
        app.run = RunState::Idle;
        app.pending_tools.clear();
        let (cr, _) = live_rows(&app, 50, 10);
        terminal.draw(|frame| draw(frame, &app, &[], cr)).unwrap();
        let text = buffer_text(&terminal);
        assert!(!text.contains("thinking"), "{text}");
    }

    #[test]
    fn footer_swaps_to_armed_hints() {
        let mut app = App::new();
        app.hint = Some(Hint::QuitArmed);
        // The hint replaces the cwd corner on the first footer row.
        let rows = footer_lines(&app, 80);
        assert!(rows[0].to_string().contains("again to quit"));
    }

    #[test]
    fn draw_renders_the_live_streaming_cell() {
        let mut terminal = FrameTerminal::new(TestBackend::new(40, 12)).unwrap();
        let mut app = App::new();
        app.run = RunState::Running {
            started: Instant::now(),
            cancelling: false,
        };
        app.streaming = Some("streaming tokens here".into());

        let (cr, _) = live_rows(&app, 40, 12);
        terminal.draw(|frame| draw(frame, &app, &[], cr)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("streaming tokens"), "live cell text: {text}");
        assert!(
            text.contains('●'),
            "bulleted like an assistant message: {text}"
        );
    }

    #[test]
    fn streaming_cell_fills_available_height_not_a_fixed_window() {
        // Regression: the live cell used to hard-cap at 12 rows regardless of
        // terminal height, so a long turn looked "cut" on a tall screen. It now
        // scales with the available live-region height and always shows the tail.
        let mut app = App::new();
        app.run = RunState::Running {
            started: Instant::now(),
            cancelling: false,
        };
        app.streaming = Some(
            (0..30)
                .map(|i| format!("- line{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let tall = streaming_cell(&app, 60, 40, 3);
        let short = streaming_cell(&app, 60, 10, 3);
        assert!(
            tall.len() > 12,
            "a tall terminal shows more than the old fixed 12 rows: {}",
            tall.len()
        );
        assert!(
            short.len() < tall.len(),
            "shorter screen → fewer visible rows: {} vs {}",
            short.len(),
            tall.len()
        );
        // Both always show the latest content (the tail), not the head.
        assert!(
            tall.last().unwrap().to_string().contains("line29"),
            "tall shows the tail"
        );
        assert!(
            short.last().unwrap().to_string().contains("line29"),
            "short shows the tail"
        );
    }
}
