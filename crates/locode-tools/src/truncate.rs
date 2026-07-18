//! The shared model-facing truncation post-process (ADR-0008 amendment
//! 2026-07-18: applied centrally at the dispatch door — every `tool_result`
//! passes through [`truncate_for_model`] in `Registry::dispatch`, so no tool
//! can flood the model regardless of its own caps). Moved from `locode-host`
//! (where nothing consumed it): the budget is a property of the model-facing
//! boundary, not of OS access.

/// Default model-facing byte budget. Matches Grok's shell output cap.
pub const MODEL_OUTPUT_BUDGET: usize = 30_000;

/// Bound `text` to roughly `budget` bytes for model consumption, keeping the **head and
/// tail** and eliding the middle (middle-truncation, Codex-style) with a byte-count
/// marker in the seam.
///
/// Returns `(text, was_truncated)`. Below budget the input is returned untouched with no
/// marker (idempotent). UTF-8-safe: cuts fall on char boundaries.
#[must_use]
pub fn truncate_for_model(text: &str, budget: usize) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_string(), false);
    }

    let head_len = budget / 2;
    let tail_len = budget - head_len;
    let head_end = floor_boundary(text, head_len);
    let tail_start = ceil_boundary(text, text.len() - tail_len);
    let elided = tail_start.saturating_sub(head_end);

    let marker = format!("\n[... {elided} bytes truncated ...]\n");
    let mut out = String::with_capacity(head_end + marker.len() + (text.len() - tail_start));
    out.push_str(&text[..head_end]);
    out.push_str(&marker);
    out.push_str(&text[tail_start..]);
    (out, true)
}

/// The largest char boundary `<= idx`.
fn floor_boundary(text: &str, mut idx: usize) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// The smallest char boundary `>= idx`.
fn ceil_boundary(text: &str, mut idx: usize) -> usize {
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_budget_is_untouched() {
        let (out, truncated) = truncate_for_model("short", 100);
        assert_eq!(out, "short");
        assert!(!truncated);
    }

    #[test]
    fn over_budget_keeps_head_and_tail() {
        let text: String = (0..1000).map(|_| 'x').collect::<String>() + "TAILMARK";
        let head: String = std::iter::repeat_n('H', 500).collect();
        let text = head.clone() + &text;
        let (out, truncated) = truncate_for_model(&text, 200);
        assert!(truncated);
        assert!(out.starts_with("HHHH"), "keeps the head");
        assert!(out.ends_with("TAILMARK"), "keeps the tail");
        assert!(out.contains("truncated"), "has the marker");
        assert!(out.len() < text.len());
    }

    #[test]
    fn utf8_multibyte_seam_does_not_panic() {
        // A string of multibyte chars; every cut must land on a boundary.
        let text: String = std::iter::repeat_n('あ', 10_000).collect();
        let (out, truncated) = truncate_for_model(&text, 1000);
        assert!(truncated);
        // Round-trips as valid UTF-8 (the pushes would have panicked otherwise).
        assert!(out.starts_with('あ') && out.ends_with('あ'));
    }
}
