//! Rendering: the live region (bottom-anchored) and its widgets.
//!
//! The live region is the ONLY repainted surface (SPEC-TUI rendering model);
//! finalized transcript blocks are printed once into native scrollback
//! (slice 2). Blank rows above the composer read as margin.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::{App, Hint};

pub mod composer;

/// Draw the live region: flexible blank space, then composer, then footer.
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let composer_height = app.composer.desired_height(frame.area().width);
    let [_, composer_area, footer_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(composer_height),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    app.composer.render(frame, composer_area);
    frame.render_widget(Paragraph::new(footer_line(app)), footer_area);
}

fn footer_line(app: &App) -> Line<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let text = match app.hint {
        Some(Hint::QuitArmed) => "press ctrl+c again to quit".to_string(),
        Some(Hint::ClearArmed) => "press esc again to clear".to_string(),
        None => "enter to send · alt+enter newline · ctrl+c quit".to_string(),
    };
    Line::styled(text, dim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
    fn draw_renders_composer_bottom_anchored_with_footer() {
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut app = App::new();
        app.composer.insert_text("hello tui");

        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = buffer_text(&terminal);

        assert!(text.contains("❯ hello tui"), "typed text visible: {text}");
        assert!(
            text.contains("enter to send"),
            "footer hints on the last row: {text}"
        );
        // Bottom-anchored: the first rows stay blank (margin).
        let first_row: String = text.lines().next().unwrap().trim().to_string();
        assert!(first_row.is_empty(), "top row is margin: {first_row:?}");
    }

    #[test]
    fn footer_swaps_to_armed_hints() {
        let mut app = App::new();
        app.hint = Some(Hint::QuitArmed);
        let line = footer_line(&app);
        assert!(line.to_string().contains("again to quit"));
    }
}
