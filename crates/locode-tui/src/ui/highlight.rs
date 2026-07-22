//! Code-block syntax highlighting via `syntect` + `two-face` (markdown study
//! Phase 1). A compact take on codex's `render/highlight.rs`:
//!
//! - a single fixed theme, `base16-256`, which encodes colors as ANSI palette
//!   indices (not hard RGB) so highlighted code adapts to the user's terminal
//!   theme and reads on both light and dark backgrounds;
//! - **backgrounds are omitted** so the terminal background shows through, and
//!   **italic/underline are suppressed** (many terminals render them poorly, and
//!   some themes underline type scopes) — codex's choices;
//! - size guardrails: past 512 KB or 10 000 lines, return `None` (the caller
//!   falls back to plain dim text).
//!
//! `two_face::syntax::extra_newlines()` is bat's ~250-language grammar set.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, FontStyle, Style as SyntectStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::EmbeddedThemeName;

/// Skip highlighting past these caps (codex's guardrails).
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

/// Syntect/bat encode ANSI palette semantics in the alpha channel: `a = 0` means
/// the red byte is an ANSI palette index (not RGB); `a = 1` means terminal
/// default. Any other alpha is a real RGB color.
const ANSI_ALPHA_INDEX: u8 = 0x00;
const ANSI_ALPHA_DEFAULT: u8 = 0x01;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(EmbeddedThemeName::Base16_256)
            .clone()
    })
}

/// Highlight `code` as `lang`, returning one styled-span vector per line, or
/// `None` when the language is unknown or the input is too large (the caller
/// then renders plain text). `code` should not include the fence markers.
#[must_use]
pub fn highlight_lines(code: &str, lang: &str) -> Option<Vec<Vec<Span<'static>>>> {
    if code.is_empty()
        || lang.is_empty()
        || code.len() > MAX_HIGHLIGHT_BYTES
        || code.lines().count() > MAX_HIGHLIGHT_LINES
    {
        return None;
    }
    let syntax = find_syntax(lang)?;
    let mut h = HighlightLines::new(syntax, theme());
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    for line in LinesWithEndings::from(code) {
        let ranges = h.highlight_line(line, syntax_set()).ok()?;
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (style, text) in ranges {
            // We handle line breaks ourselves; drop trailing LF/CR.
            let text = text.trim_end_matches(['\n', '\r']);
            if !text.is_empty() {
                spans.push(Span::styled(text.to_string(), convert_style(style)));
            }
        }
        lines.push(spans);
    }
    Some(lines)
}

/// Resolve a language token to a syntax, with a few aliases two-face misses.
fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();
    let normalized = lang.to_ascii_lowercase();
    let patched = match normalized.as_str() {
        "csharp" | "c-sharp" => "c#",
        "golang" => "go",
        "python3" => "python",
        "shell" | "sh" => "bash",
        other => other,
    };
    ss.find_syntax_by_token(patched)
        .or_else(|| ss.find_syntax_by_name(patched))
        .or_else(|| ss.find_syntax_by_extension(lang))
}

/// Convert a syntect style to a ratatui style: foreground only, bold kept,
/// italic/underline dropped, background left to the terminal.
fn convert_style(syn: SyntectStyle) -> Style {
    let mut style = Style::default();
    if let Some(fg) = convert_color(syn.foreground) {
        style = style.fg(fg);
    }
    if syn.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

/// Decode a syntect color, honoring the bat ANSI-palette alpha encoding so the
/// `base16-256` theme maps to the terminal's own palette.
fn convert_color(color: SyntectColor) -> Option<Color> {
    match color.a {
        ANSI_ALPHA_INDEX => Some(ansi_palette_color(color.r)),
        ANSI_ALPHA_DEFAULT => None,
        _ => Some(Color::Rgb(color.r, color.g, color.b)),
    }
}

/// Map a low ANSI index to a named color (terminal-theme-relative), higher
/// indices to the 256-color cube.
fn ansi_palette_color(index: u8) -> Color {
    match index {
        0x00 => Color::Black,
        0x01 => Color::Red,
        0x02 => Color::Green,
        0x03 => Color::Yellow,
        0x04 => Color::Blue,
        0x05 => Color::Magenta,
        0x06 => Color::Cyan,
        0x07 => Color::Gray,
        n => Color::Indexed(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reconstructed(lines: &[Vec<Span<'_>>]) -> String {
        lines
            .iter()
            .map(|spans| spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn highlights_known_language_and_preserves_content() {
        let code = "fn main() {\n    let x = 1;\n}";
        let lines = highlight_lines(code, "rust").expect("rust highlights");
        assert_eq!(lines.len(), 3);
        assert_eq!(reconstructed(&lines), code, "content byte-preserved");
        // At least one span carries a color (something was highlighted).
        assert!(
            lines.iter().flatten().any(|s| s.style.fg.is_some()),
            "some fg color applied"
        );
    }

    #[test]
    fn resolves_common_aliases() {
        for lang in [
            "rust", "rs", "python", "py", "bash", "sh", "shell", "js", "csharp",
        ] {
            assert!(find_syntax(lang).is_some(), "no syntax for {lang:?}");
        }
    }

    #[test]
    fn unknown_or_empty_language_falls_back() {
        assert!(highlight_lines("x = 1", "no-such-lang").is_none());
        assert!(highlight_lines("plain", "").is_none());
        assert!(highlight_lines("", "rust").is_none());
    }

    #[test]
    fn oversized_input_falls_back() {
        let big = "x\n".repeat(MAX_HIGHLIGHT_LINES + 1);
        assert!(highlight_lines(&big, "rust").is_none());
    }
}
