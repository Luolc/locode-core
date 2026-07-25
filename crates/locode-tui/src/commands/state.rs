//! What the composer text implies about the command menu.
//!
//! Grok's split (`slash/mod.rs`): a **controller** derives an immutable
//! **snapshot** from `(text, cursor)` on every edit, and the renderer only reads the
//! snapshot. We collapse the two into one struct — our reducer already owns all UI
//! state and re-derives on each keystroke — but keep the important part: nothing here
//! reads the terminal, so every rule below is table-testable.
//!
//! Scope of this module is the **command** phase. Argument suggestions (grok's second
//! menu) arrive with `suggest_args`; until then a cursor past the command token simply
//! closes the menu, which is what the finished behavior does too when a command offers
//! no argument rows.

use std::ops::Range;

use super::command::CommandCtx;
use super::registry::{CommandRegistry, CommandTrigger};

/// Dropdown rows visible at once before it scrolls (grok's
/// `MAX_VISIBLE_SUGGESTIONS`).
pub const MAX_VISIBLE_ROWS: usize = 6;

/// One dropdown row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionRow {
    /// Label, with the leading slash (`/model`).
    pub display: String,
    /// The dimmed second column.
    pub description: String,
    /// What replaces the command token on acceptance.
    ///
    /// A **trailing space means "more input expected"** — grok's chaining signal
    /// (`agent_view/prompt.rs:205-212`): Enter on such a row completes the text and
    /// leaves the menu open instead of submitting.
    pub insert_text: String,
    /// Character positions of `display` that matched the query, for highlighting.
    /// Empty until the fuzzy matcher lands.
    pub indices: Vec<u32>,
}

impl SuggestionRow {
    /// The row for a command trigger. `takes_args` earns the trailing space.
    fn from_trigger(trigger: &CommandTrigger, indices: Vec<u32>) -> Self {
        let mut insert_text = trigger.display.clone();
        if trigger.takes_args {
            insert_text.push(' ');
        }
        Self {
            display: trigger.display.clone(),
            description: trigger.description.clone(),
            insert_text,
            indices,
        }
    }
}

/// The menu state derived from the composer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlashState {
    /// Whether the dropdown should be drawn.
    pub open: bool,
    /// The command name typed so far, without the slash and clamped to the cursor.
    pub query: String,
    /// Rows to draw, best first.
    pub matches: Vec<SuggestionRow>,
    /// Selected index into `matches`.
    pub selected: usize,
    /// Character range of the command token (`0..6` for `/model`), which is what
    /// acceptance replaces.
    pub command_range: Option<Range<usize>>,
    /// The draft the user pressed Esc on.
    ///
    /// Everything else here is a pure function of the composer, so without this the
    /// menu would re-open on the very next refresh — Esc would do nothing visible.
    /// Keyed on the **text**, matching grok, whose refresh runs on edits only: moving
    /// the cursor around a dismissed draft leaves it dismissed; editing it re-derives.
    dismissed: Option<String>,
}

impl SlashState {
    /// Re-derive from the composer's `text` and character-offset `cursor`.
    ///
    /// The selection is carried across the refresh when the same row is still present
    /// (grok's `carry_selection`), so typing another letter does not silently jump the
    /// highlight to an unrelated command.
    pub fn refresh(
        &mut self,
        registry: &CommandRegistry,
        ctx: &CommandCtx<'_>,
        text: &str,
        cursor: usize,
    ) {
        if self.dismissed.as_deref() == Some(text) {
            return;
        }
        let previous = std::mem::take(self);
        let Some(input) = analyze_input(text, cursor) else {
            return;
        };
        if !input.cursor_in_command {
            // The argument phase; nothing to offer until `suggest_args` is wired.
            return;
        }
        let matches = command_rows(registry, ctx, &input.query);
        self.selected = carry_selection(&previous, &matches, &input.query);
        self.open = !matches.is_empty();
        self.matches = matches;
        self.query = input.query;
        self.command_range = Some(0..input.command_end);
    }

    /// An open menu over `rows` — a fixture for the renderer's tests, which have no
    /// registry to derive a real state from.
    #[cfg(test)]
    #[must_use]
    pub fn open_with(rows: Vec<SuggestionRow>, selected: usize) -> Self {
        Self {
            open: true,
            matches: rows,
            selected,
            command_range: Some(0..1),
            ..Self::default()
        }
    }

    /// Close the menu, keeping nothing.
    pub fn close(&mut self) {
        *self = Self::default();
    }

    /// Close the menu and keep it closed for this exact draft (the Esc gesture).
    pub fn dismiss(&mut self, text: &str) {
        *self = Self {
            dismissed: Some(text.to_owned()),
            ..Self::default()
        };
    }

    /// The selected row.
    #[must_use]
    pub fn selection(&self) -> Option<&SuggestionRow> {
        self.matches
            .get(self.selected.min(self.matches.len().checked_sub(1)?))
    }

    /// Move the selection by `delta`, **wrapping** at both ends (grok's
    /// `move_selection`: a menu this short is faster to wrap than to clamp).
    pub fn move_selection(&mut self, delta: isize) {
        let len = self.matches.len();
        if len == 0 {
            return;
        }
        let len_i = isize::try_from(len).unwrap_or(isize::MAX);
        let current = isize::try_from(self.selected.min(len - 1)).unwrap_or(0);
        self.selected = usize::try_from((current + delta).rem_euclid(len_i)).unwrap_or(0);
    }

    /// The text edit accepting the selection implies: `(range_to_replace,
    /// replacement)` in **character** offsets, or `None` when there is nothing to
    /// accept.
    #[must_use]
    pub fn accept(&self) -> Option<(Range<usize>, String)> {
        let range = self.command_range.clone()?;
        let row = self.selection()?;
        Some((range, row.insert_text.clone()))
    }

    /// Whether accepting the selection expects more typing (the row's trailing
    /// space) — in which case Enter completes without submitting.
    #[must_use]
    pub fn selection_chains(&self) -> bool {
        self.selection()
            .is_some_and(|row| row.insert_text.ends_with(' '))
    }

    /// The first visible row index for a viewport of `height` rows, keeping the
    /// selection centred once the list scrolls (grok's dropdown scroll rule).
    #[must_use]
    pub fn scroll_offset(&self, height: usize) -> usize {
        let total = self.matches.len();
        if height == 0 || total <= height {
            return 0;
        }
        let selected = self.selected.min(total - 1);
        if selected < height / 2 {
            0
        } else if selected + height / 2 >= total {
            total - height
        } else {
            selected - height / 2
        }
    }
}

/// What `analyze_input` extracts from the composer.
struct Input {
    /// Character offset just past the command token.
    command_end: usize,
    /// The command name typed so far, clamped to the cursor.
    query: String,
    /// Whether the cursor is still inside the command token.
    cursor_in_command: bool,
}

/// Split the composer text around the cursor (grok's `analyze_input`,
/// `slash/mod.rs:1041`).
///
/// `None` — no menu at all — when the text is not a single line beginning with `/`.
/// The single-line rule is ours: a `/` at the start of a *multiline* draft is far more
/// likely to be pasted content than a command, and the composer explicitly supports
/// multiline drafts (Alt+Enter).
fn analyze_input(text: &str, cursor: usize) -> Option<Input> {
    if !text.starts_with('/') || text.contains('\n') {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let command_end = chars
        .iter()
        .skip(1)
        .position(|c| c.is_whitespace())
        .map_or(chars.len(), |i| i + 1);
    // The query is clamped to the cursor, so `/` typed *before* existing text offers
    // the full list rather than matching against text the user has not reached yet.
    let query_end = cursor.clamp(1, command_end);
    Some(Input {
        command_end,
        query: chars[1..query_end].iter().collect(),
        cursor_in_command: cursor <= command_end,
    })
}

/// The rows for a command-phase `query`.
///
/// An empty query lists every visible command **once**, by canonical name — an alias
/// row would be a second entry for something already listed (grok dedups by command
/// for the same reason). With a query, aliases compete on their own.
fn command_rows(
    registry: &CommandRegistry,
    ctx: &CommandCtx<'_>,
    query: &str,
) -> Vec<SuggestionRow> {
    let visible = registry.visible_triggers(ctx);
    let query = query.trim();
    if query.is_empty() {
        return visible
            .into_iter()
            .filter(|t| t.alias.is_none())
            .map(|t| SuggestionRow::from_trigger(t, Vec::new()))
            .collect();
    }
    // A second slash is never part of a command name — `/usr/bin` must not light up
    // the menu on its way to being ordinary text.
    if query.contains('/') {
        return Vec::new();
    }
    rank(&visible, query)
        .into_iter()
        .map(|(trigger, indices)| SuggestionRow::from_trigger(trigger, indices))
        .collect()
}

/// Rank visible triggers against a non-empty `query`.
///
/// Substring matching for now — prefix hits first, then by source (builtins ahead of
/// skills), then alphabetically. The fuzzy matcher replaces the body of this function
/// and nothing else: everything above consumes `(trigger, indices)`.
fn rank<'a>(triggers: &[&'a CommandTrigger], query: &str) -> Vec<(&'a CommandTrigger, Vec<u32>)> {
    let needle = query.to_lowercase();
    let mut hits: Vec<(&CommandTrigger, Vec<u32>, bool)> = triggers
        .iter()
        .filter_map(|t| {
            // Indices are positions in `display` (`/model`), which is what the renderer
            // highlights; the leading slash shifts them by one.
            let hay = t.match_text.to_lowercase();
            let at = hay.find(&needle)?;
            let start = hay[..at].chars().count() + 1;
            let len = needle.chars().count();
            let indices = (start..start + len)
                .map(|i| u32::try_from(i).unwrap_or(u32::MAX))
                .collect();
            Some((*t, indices, at == 0))
        })
        .collect();
    hits.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.source.cmp(&b.0.source))
            .then_with(|| a.0.display.cmp(&b.0.display))
    });
    hits.into_iter().map(|(t, i, _)| (t, i)).collect()
}

/// Keep the highlight on the same row across a refresh when it survived; otherwise
/// start at the top. Matching on `insert_text` (grok's key) rather than the index is
/// what stops the highlight from sliding onto a neighbour as rows are filtered out.
fn carry_selection(previous: &SlashState, matches: &[SuggestionRow], query: &str) -> usize {
    if matches.is_empty() || previous.matches.is_empty() || previous.query == query {
        return previous.selected.min(matches.len().saturating_sub(1));
    }
    let previous_row = &previous.matches[previous.selected.min(previous.matches.len() - 1)];
    matches
        .iter()
        .position(|row| row.insert_text == previous_row.insert_text)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::command::{CommandResult, SlashCommand};
    use crate::commands::registry::CommandSource;
    use std::sync::Arc;

    struct Fake {
        name: &'static str,
        aliases: Vec<&'static str>,
        takes: bool,
    }

    impl Fake {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                aliases: Vec::new(),
                takes: false,
            }
        }
        fn alias(mut self, a: &'static str) -> Self {
            self.aliases.push(a);
            self
        }
        fn takes_args(mut self) -> Self {
            self.takes = true;
            self
        }
    }

    #[async_trait::async_trait]
    impl SlashCommand for Fake {
        fn name(&self) -> &str {
            self.name
        }
        fn aliases(&self) -> &[&str] {
            &self.aliases
        }
        fn description(&self) -> &'static str {
            "does a thing"
        }
        fn usage(&self) -> &'static str {
            "/fake"
        }
        fn takes_args(&self) -> bool {
            self.takes
        }
        async fn execute(&self, _c: &CommandCtx<'_>, _a: &str) -> CommandResult {
            CommandResult::Handled
        }
    }

    fn registry(names: Vec<Fake>) -> CommandRegistry {
        let mut r = CommandRegistry::new();
        for f in names {
            r.register(Arc::new(f), CommandSource::Builtin);
        }
        r
    }

    /// `text` with the cursor at its end.
    fn state(registry: &CommandRegistry, text: &str) -> SlashState {
        let mut s = SlashState::default();
        s.refresh(registry, &CommandCtx::default(), text, text.chars().count());
        s
    }

    fn labels(s: &SlashState) -> Vec<&str> {
        s.matches.iter().map(|r| r.display.as_str()).collect()
    }

    #[test]
    fn a_bare_slash_opens_the_full_list_by_canonical_name() {
        let r = registry(vec![Fake::new("new"), Fake::new("quit").alias("exit")]);
        let s = state(&r, "/");
        assert!(s.open);
        assert_eq!(
            labels(&s),
            vec!["/new", "/quit"],
            "aliases do not double-list a command they already name"
        );
        assert_eq!(s.query, "");
    }

    #[test]
    fn typing_narrows_and_prefix_hits_come_first() {
        let r = registry(vec![
            Fake::new("new"),
            Fake::new("quit"),
            Fake::new("renew"),
        ]);
        let s = state(&r, "/new");
        assert_eq!(
            labels(&s),
            vec!["/new", "/renew"],
            "prefix match ahead of the substring match"
        );
        assert_eq!(s.query, "new");
    }

    #[test]
    fn no_match_closes_the_menu() {
        let r = registry(vec![Fake::new("new")]);
        let s = state(&r, "/zzz");
        assert!(!s.open);
        assert!(s.matches.is_empty());
    }

    /// Ordinary text — including a leading path — must never open the menu.
    #[test]
    fn non_command_text_never_opens_the_menu() {
        let r = registry(vec![Fake::new("new")]);
        assert!(!state(&r, "hello").open, "no slash");
        assert!(!state(&r, "/usr/bin/env").open, "a path is not a command");
        assert!(
            !state(&r, "/new\nsecond line").open,
            "a multiline draft is content, not a command"
        );
    }

    /// Past the command token there is nothing to offer yet, so the menu closes.
    #[test]
    fn the_cursor_leaving_the_command_token_closes_the_menu() {
        let r = registry(vec![Fake::new("new")]);
        assert!(!state(&r, "/new ").open);
    }

    /// The query is clamped to the cursor: `/` typed in front of existing text lists
    /// everything rather than matching against text the user has not reached.
    #[test]
    fn the_query_is_clamped_to_the_cursor() {
        let r = registry(vec![Fake::new("new"), Fake::new("quit")]);
        let mut s = SlashState::default();
        s.refresh(&r, &CommandCtx::default(), "/quit", 1);
        assert_eq!(s.query, "");
        assert_eq!(labels(&s), vec!["/new", "/quit"]);
    }

    /// Esc must stick: everything else here is derived from the composer, so without
    /// the dismissal the very next refresh would re-open the menu.
    #[test]
    fn dismissal_survives_refreshes_of_the_same_draft_and_ends_at_the_next_edit() {
        let r = registry(vec![Fake::new("new"), Fake::new("quit")]);
        let mut s = state(&r, "/n");
        assert!(s.open);

        s.dismiss("/n");
        s.refresh(&r, &CommandCtx::default(), "/n", 2);
        assert!(!s.open, "still dismissed for the same draft");
        s.refresh(&r, &CommandCtx::default(), "/n", 1);
        assert!(!s.open, "moving the cursor does not undo the dismissal");

        s.refresh(&r, &CommandCtx::default(), "/ne", 3);
        assert!(s.open, "editing re-derives the menu");
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let r = registry(vec![Fake::new("a"), Fake::new("b"), Fake::new("c")]);
        let mut s = state(&r, "/");
        assert_eq!(s.selected, 0);
        s.move_selection(1);
        assert_eq!(s.selected, 1);
        s.move_selection(-1);
        assert_eq!(s.selected, 0);
        s.move_selection(-1);
        assert_eq!(s.selected, 2, "up from the top wraps to the bottom");
        s.move_selection(1);
        assert_eq!(s.selected, 0, "down from the bottom wraps to the top");
    }

    /// The highlight follows the row, not the index: narrowing the list must not
    /// silently move the selection onto a different command.
    #[test]
    fn the_selection_follows_its_row_across_a_refresh() {
        let r = registry(vec![Fake::new("new"), Fake::new("quit")]);
        let mut s = state(&r, "/");
        s.move_selection(1); // "/quit"
        assert_eq!(s.selection().unwrap().display, "/quit");
        s.refresh(&r, &CommandCtx::default(), "/qu", 3);
        assert_eq!(
            s.selection().unwrap().display,
            "/quit",
            "still on /quit after the list narrowed"
        );
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn accept_replaces_the_command_token_and_chains_only_when_args_follow() {
        let r = registry(vec![Fake::new("model").takes_args(), Fake::new("quit")]);
        let s = state(&r, "/mod");
        let (range, text) = s.accept().expect("accepts");
        assert_eq!(range, 0..4);
        assert_eq!(
            text, "/model ",
            "a command taking args completes with a space"
        );
        assert!(s.selection_chains(), "trailing space = more input expected");

        let s = state(&r, "/qui");
        assert_eq!(s.accept().unwrap().1, "/quit");
        assert!(!s.selection_chains());
    }

    /// The viewport keeps the selection centred once the list is longer than it.
    #[test]
    fn scroll_keeps_the_selection_centred() {
        let r = registry(vec![
            Fake::new("a"),
            Fake::new("b"),
            Fake::new("c"),
            Fake::new("d"),
            Fake::new("e"),
        ]);
        let mut s = state(&r, "/");
        assert_eq!(s.scroll_offset(3), 0, "top of a fresh list");
        s.selected = 2;
        assert_eq!(s.scroll_offset(3), 1, "centred");
        s.selected = 4;
        assert_eq!(s.scroll_offset(3), 2, "pinned at the bottom");
        assert_eq!(s.scroll_offset(9), 0, "no scroll when everything fits");
    }

    #[test]
    fn matched_letters_are_reported_as_display_positions() {
        let r = registry(vec![Fake::new("model")]);
        let s = state(&r, "/od");
        // "/model": `o` at 2, `d` at 4 — the slash shifts every index by one.
        assert_eq!(s.matches[0].indices, vec![2, 3]);
        assert_eq!(&s.matches[0].display[2..4], "od");
    }
}
