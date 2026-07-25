//! What the composer text implies about the command menu.
//!
//! Grok's split (`slash/mod.rs`): a **controller** derives an immutable
//! **snapshot** from `(text, cursor)` on every edit, and the renderer only reads the
//! snapshot. We collapse the two into one struct — our reducer already owns all UI
//! state and re-derives on each keystroke — but keep the important part: nothing here
//! reads the terminal, so every rule below is table-testable.
//!
//! Two phases, one state: while the cursor is inside the command token the menu offers
//! **commands**; once it moves past it, the recognized command's own `suggest_args`
//! offers **arguments** (grok's second-level menu). A command with nothing to suggest
//! simply closes the menu.

use std::ops::Range;

use super::command::{ArgItem, CommandCtx};
use super::matcher::FuzzyMatcher;
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

/// Reduce a description to a one-paragraph, single-line summary for the menu.
///
/// A skill's description is markdown written for a model, so it routinely runs to
/// several paragraphs and its own bullet list. Two steps, in order:
///
/// 1. **Keep the first paragraph.** A description's opening paragraph is its summary;
///    what follows is detail the menu has no room for, and rendering a bullet list
///    inline reads as gibberish.
/// 2. **Collapse the remaining whitespace.** A hard-wrapped first paragraph reflows to
///    one line, so the renderer's own wrapping decides the line breaks rather than
///    inheriting the author's.
///
/// Cutting at the paragraph break rather than the first newline is what keeps a
/// hard-wrapped opening sentence intact.
fn summarize(description: &str) -> String {
    let first_paragraph = description
        .split("\n\n")
        .find(|p| !p.trim().is_empty())
        .unwrap_or(description);
    first_paragraph
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
            description: summarize(&trigger.description),
            insert_text,
            indices,
        }
    }

    /// The row for an argument suggestion. The item's three texts stay separate all the
    /// way to the screen: `display` is shown, `match_text` was ranked, `insert_text` is
    /// written.
    fn from_arg(item: &ArgItem, indices: Vec<u32>) -> Self {
        Self {
            display: item.display.clone(),
            description: summarize(&item.description),
            insert_text: item.insert_text.clone(),
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
    /// acceptance replaces in the command phase.
    pub command_range: Option<Range<usize>>,
    /// Character range of the argument text, which acceptance replaces in the argument
    /// phase.
    pub args_range: Option<Range<usize>>,
    /// Which phase the menu is in: `true` = offering commands, `false` = arguments.
    pub cursor_in_command: bool,
    /// Dim text the composer draws at the caret, from one of two sources (grok keeps
    /// them separate too, `views/prompt_widget/mod.rs:3030-3068`):
    ///
    /// - **command phase** — the rest of the selected command's name, so `/comm` with
    ///   `commit` selected shows `it`. Only when the typed text is a genuine *prefix*
    ///   of it: a fuzzy hit like `/mdl` → `model` has no suffix to offer.
    /// - **argument phase** — the command's [`crate::commands::SlashCommand::arg_placeholder`], while no
    ///   argument has been typed yet.
    ///
    /// Set independently of `open`: a command with no argument suggestions still shows
    /// what it expects.
    pub ghost: Option<String>,
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
        matcher: &mut FuzzyMatcher,
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
        let (rows, query) = if input.cursor_in_command {
            let rows = command_rows(registry, matcher, ctx, &input.query);
            (rows, input.query.clone())
        } else {
            let rows = arg_rows(registry, matcher, ctx, text, &input.args_query);
            (rows, input.args_query.clone())
        };
        self.selected = carry_selection(&previous, &rows, &query, input.cursor_in_command);
        self.open = !rows.is_empty();
        self.matches = rows;
        self.query = query;
        self.cursor_in_command = input.cursor_in_command;
        self.command_range = Some(0..input.command_end);
        self.args_range = input.args_range;
        self.ghost = if input.cursor_in_command {
            // Only when the token ends the line: the hint is drawn at the caret, and
            // there is nothing there to overwrite.
            (input.command_end == text.chars().count())
                .then(|| self.name_suffix())
                .flatten()
        } else {
            argument_hint(registry, text, &input.args_query)
        };
    }

    /// The rest of the selected command's name after what has been typed.
    ///
    /// `None` unless the query is a genuine prefix of it (grok's
    /// `command_prefix_matches_smart`): the ranking is fuzzy, so the selected row may
    /// match letters scattered through the name, and there is no "rest" to offer then.
    fn name_suffix(&self) -> Option<String> {
        let name = self.selection()?.display.strip_prefix('/')?;
        if self.query.is_empty() || !smart_prefix(name, &self.query) {
            return None;
        }
        let suffix: String = name.chars().skip(self.query.chars().count()).collect();
        (!suffix.is_empty()).then_some(suffix)
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
        let range = if self.cursor_in_command {
            self.command_range.clone()?
        } else {
            self.args_range.clone()?
        };
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
    /// Character range of the argument text (`None` while the cursor is in the command
    /// token). Runs to the end of the line, as grok's does: accepting an argument
    /// replaces the whole tail rather than splicing into it.
    args_range: Option<Range<usize>>,
    /// The argument text up to the cursor, which is what argument rows are ranked on.
    args_query: String,
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
    let cursor_in_command = cursor <= command_end;
    let (args_range, args_query) = if cursor_in_command {
        (None, String::new())
    } else {
        let start = chars[command_end..]
            .iter()
            .position(|c| !c.is_whitespace())
            .map_or(chars.len(), |i| command_end + i);
        let end = chars.len();
        let query_end = cursor.clamp(start, end);
        (
            Some(start..end),
            chars[start.min(query_end)..query_end].iter().collect(),
        )
    };
    Some(Input {
        command_end,
        query: chars[1..query_end].iter().collect(),
        cursor_in_command,
        args_range,
        args_query,
    })
}

/// Whether `query` is a prefix of `name` under the matcher's smart-case rule: an
/// all-lowercase query ignores case, any uppercase character demands an exact match
/// (grok's `command_prefix_matches_smart`).
fn smart_prefix(name: &str, query: &str) -> bool {
    if query.chars().any(char::is_uppercase) {
        return name.starts_with(query);
    }
    let mut chars = name.chars();
    query
        .chars()
        .all(|q| chars.next().is_some_and(|n| n.eq_ignore_ascii_case(&q)))
}

/// The hint for a recognized command that has been given no argument yet.
fn argument_hint(registry: &CommandRegistry, text: &str, query: &str) -> Option<String> {
    if !query.trim().is_empty() {
        return None;
    }
    let (command, _) = registry.resolve(text).ok()?;
    command.arg_placeholder().map(str::to_string)
}

/// The rows for the argument phase: whatever the recognized command suggests, ranked
/// against what has been typed after it.
///
/// Empty when the line names no command or the command has no suggestions — which is
/// what closes the menu the moment the cursor leaves `/new`.
fn arg_rows(
    registry: &CommandRegistry,
    matcher: &mut FuzzyMatcher,
    ctx: &CommandCtx<'_>,
    text: &str,
    query: &str,
) -> Vec<SuggestionRow> {
    let Ok((command, _)) = registry.resolve(text) else {
        return Vec::new();
    };
    let Some(items) = command.suggest_args(ctx, query) else {
        return Vec::new();
    };
    if query.trim().is_empty() {
        return items
            .iter()
            .map(|item| SuggestionRow::from_arg(item, Vec::new()))
            .collect();
    }
    matcher
        .rank(&items, query, |item| item.match_text.as_str())
        .into_iter()
        .map(|(i, _)| {
            let item = &items[i];
            let indices = matcher.indices(&item.display);
            SuggestionRow::from_arg(item, indices)
        })
        .collect()
}

/// The rows for a command-phase `query`.
///
/// An empty query lists every visible command **once**, by canonical name — an alias
/// row would be a second entry for something already listed (grok dedups by command
/// for the same reason). With a query, aliases compete on their own.
fn command_rows(
    registry: &CommandRegistry,
    matcher: &mut FuzzyMatcher,
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
    rank(&visible, matcher, query)
        .into_iter()
        .map(|(trigger, indices)| SuggestionRow::from_trigger(trigger, indices))
        .collect()
}

/// Rank visible triggers against a non-empty `query`.
///
/// Fuzzy, so `/mdl` finds `model`. Scoring runs on `match_text` (the bare name) while
/// the highlight indices are taken for `display` (`/model`) — the same two calls in
/// grok's `command_suggestions`, and the reason the leading slash never has to be
/// accounted for by hand.
///
/// **One row per command.** An alias and its canonical name are separate triggers, so
/// `/e` could otherwise list `/exit` and `/quit` as if they were two commands; the
/// better trigger wins, preferring an exact match, then the canonical name (grok's
/// tiebreak chain).
fn rank<'a>(
    triggers: &[&'a CommandTrigger],
    matcher: &mut FuzzyMatcher,
    query: &str,
) -> Vec<(&'a CommandTrigger, Vec<u32>)> {
    let hits = matcher.rank(triggers, query, |t| t.match_text.as_str());
    let mut best: Vec<(usize, u32)> = Vec::new();
    for (i, score) in hits {
        let trigger = triggers[i];
        match best
            .iter_mut()
            .find(|(j, _)| triggers[*j].command_index == trigger.command_index)
        {
            Some(slot) => {
                if beats(trigger, triggers[slot.0], score, slot.1, query) {
                    *slot = (i, score);
                }
            }
            None => best.push((i, score)),
        }
    }
    // `matcher.rank` already ordered by score then key text; keeping `best` in that
    // order preserves it, so only the per-command choice happens here.
    best.into_iter()
        .map(|(i, _)| {
            let trigger = triggers[i];
            (trigger, matcher.indices(&trigger.display))
        })
        .collect()
}

/// Whether `new` should replace `old` as the row standing for their shared command.
fn beats(
    new: &CommandTrigger,
    old: &CommandTrigger,
    new_score: u32,
    old_score: u32,
    query: &str,
) -> bool {
    if new_score != old_score {
        return new_score > old_score;
    }
    let (new_exact, old_exact) = (new.match_text == query, old.match_text == query);
    if new_exact != old_exact {
        return new_exact;
    }
    let (new_canonical, old_canonical) = (new.alias.is_none(), old.alias.is_none());
    if new_canonical != old_canonical {
        return new_canonical;
    }
    new.display < old.display
}

/// Keep the highlight on the same row across a refresh when it survived; otherwise
/// start at the top. Matching on `insert_text` (grok's key) rather than the index is
/// what stops the highlight from sliding onto a neighbour as rows are filtered out.
fn carry_selection(
    previous: &SlashState,
    matches: &[SuggestionRow],
    query: &str,
    cursor_in_command: bool,
) -> usize {
    if matches.is_empty()
        || previous.matches.is_empty()
        || previous.cursor_in_command != cursor_in_command
    {
        return 0;
    }
    if previous.query == query {
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
        args: Vec<&'static str>,
    }

    impl Fake {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                aliases: Vec::new(),
                takes: false,
                args: Vec::new(),
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
        /// Suggests these argument values, each shown with a `» ` prefix so the test can
        /// tell *shown* from *inserted*.
        fn suggesting(mut self, args: &[&'static str]) -> Self {
            self.takes = true;
            self.args = args.to_vec();
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
        fn suggest_args(&self, _c: &CommandCtx<'_>, _q: &str) -> Option<Vec<ArgItem>> {
            if self.args.is_empty() {
                return None;
            }
            Some(
                self.args
                    .iter()
                    .map(|a| ArgItem {
                        display: format!("» {a}"),
                        match_text: (*a).to_string(),
                        insert_text: (*a).to_string(),
                        description: String::new(),
                    })
                    .collect(),
            )
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
        s.refresh(
            registry,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            text,
            text.chars().count(),
        );
        s
    }

    fn labels(s: &SlashState) -> Vec<&str> {
        s.matches.iter().map(|r| r.display.as_str()).collect()
    }

    /// A skill description is markdown written for a model: the menu takes the first
    /// paragraph, reflowed to one line, so its own bullets cannot leak into the row.
    #[test]
    fn descriptions_are_reduced_to_a_one_line_summary() {
        assert_eq!(
            summarize("Critique a design.\n\n- read the doc\n- list the risks"),
            "Critique a design.",
            "the first paragraph is the summary"
        );
        assert_eq!(
            summarize("Stage the changes\nand write a message"),
            "Stage the changes and write a message",
            "a hard-wrapped opening paragraph reflows rather than being cut"
        );
        assert_eq!(summarize("  spaced   out \t text "), "spaced out text");
        assert_eq!(summarize(""), "");
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

    /// Fuzzy, not substring: the letters of the query only have to appear in order.
    #[test]
    fn a_scattered_query_still_finds_its_command() {
        let r = registry(vec![
            Fake::new("model"),
            Fake::new("compact"),
            Fake::new("new"),
        ]);
        assert_eq!(labels(&state(&r, "/mdl")), vec!["/model"]);
        assert_eq!(labels(&state(&r, "/cmpt")), vec!["/compact"]);
        assert!(
            !"model".contains("mdl"),
            "a substring matcher found neither"
        );
    }

    /// The better match sorts first even when the worse one is alphabetically ahead.
    #[test]
    fn ranking_puts_the_better_match_first() {
        let r = registry(vec![Fake::new("automodel"), Fake::new("model")]);
        assert_eq!(
            labels(&state(&r, "/mod")),
            vec!["/model", "/automodel"],
            "a match at the start outranks one buried mid-word"
        );
    }

    /// An alias and its canonical name are separate triggers but one command, so a
    /// query matching both must not list it twice.
    #[test]
    fn a_command_matched_through_two_triggers_is_listed_once() {
        let r = registry(vec![Fake::new("quit").alias("exit")]);
        // `t` hits both `quit` and `exit`.
        assert_eq!(labels(&state(&r, "/t")), vec!["/quit"], "canonical wins");
        // …unless the alias is what was actually typed.
        assert_eq!(labels(&state(&r, "/exit")), vec!["/exit"]);
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

    /// A command with nothing to suggest closes the menu once the cursor leaves it.
    #[test]
    fn the_cursor_leaving_a_command_without_suggestions_closes_the_menu() {
        let r = registry(vec![Fake::new("new")]);
        assert!(!state(&r, "/new ").open);
    }

    /// …but a command that *does* suggest switches the menu to its arguments.
    #[test]
    fn the_menu_switches_to_arguments_past_the_command_token() {
        let r = registry(vec![Fake::new("model").suggesting(&["fast", "smart"])]);
        let s = state(&r, "/model ");
        assert!(s.open);
        assert!(!s.cursor_in_command, "the argument phase");
        assert_eq!(
            labels(&s),
            vec!["» fast", "» smart"],
            "rows show `display`, not what they insert"
        );
        assert_eq!(s.query, "", "nothing typed after the command yet");
    }

    /// Argument rows are ranked on `match_text` and insert `insert_text` — the three
    /// texts stay separate all the way through.
    #[test]
    fn argument_rows_rank_on_match_text_and_insert_insert_text() {
        let r = registry(vec![Fake::new("model").suggesting(&["fast", "smart"])]);
        let s = state(&r, "/model sm");
        assert_eq!(labels(&s), vec!["» smart"]);
        let (range, text) = s.accept().expect("accepts");
        assert_eq!(range, 7..9, "the argument text, not the command token");
        assert_eq!(text, "smart", "inserts `insert_text`");
    }

    /// The highlight starts over when the menu changes phase — an argument row is not
    /// the command row that was selected a keystroke ago.
    #[test]
    fn the_selection_resets_when_the_phase_changes() {
        let r = registry(vec![
            Fake::new("model").suggesting(&["fast", "smart"]),
            Fake::new("new"),
        ]);
        let mut s = state(&r, "/");
        s.move_selection(1); // "/new"
        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/model ",
            7,
        );
        assert_eq!(s.selected, 0, "back to the top of the argument list");
        assert_eq!(s.selection().unwrap().display, "» fast");
    }

    /// An unknown command offers no argument rows — there is nothing to ask.
    #[test]
    fn an_unrecognized_command_offers_no_arguments() {
        let r = registry(vec![Fake::new("model").suggesting(&["fast"])]);
        assert!(!state(&r, "/nope arg").open);
    }

    /// The query is clamped to the cursor: `/` typed in front of existing text lists
    /// everything rather than matching against text the user has not reached.
    #[test]
    fn the_query_is_clamped_to_the_cursor() {
        let r = registry(vec![Fake::new("new"), Fake::new("quit")]);
        let mut s = SlashState::default();
        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/quit",
            1,
        );
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
        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/n",
            2,
        );
        assert!(!s.open, "still dismissed for the same draft");
        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/n",
            1,
        );
        assert!(!s.open, "moving the cursor does not undo the dismissal");

        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/ne",
            3,
        );
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
        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/qu",
            3,
        );
        assert_eq!(
            s.selection().unwrap().display,
            "/quit",
            "still on /quit after the list narrowed"
        );
        assert_eq!(s.selected, 0);
    }

    // ── Ghost text ──────────────────────────────────────────────────────────

    #[test]
    fn the_ghost_completes_the_selected_command_name() {
        let r = registry(vec![Fake::new("commit"), Fake::new("compact")]);
        let mut s = state(&r, "/comm");
        assert_eq!(s.selection().unwrap().display, "/commit");
        assert_eq!(s.ghost.as_deref(), Some("it"));

        // It follows the selection, not the ranking.
        s.move_selection(1);
        // The selection moved but the ghost is derived at refresh time, so re-derive.
        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/compa",
            6,
        );
        assert_eq!(s.ghost.as_deref(), Some("ct"));
    }

    /// A fuzzy hit is not a prefix, so there is no "rest of the name" to offer — the
    /// guard grok's `command_prefix_matches_smart` exists for.
    #[test]
    fn a_scattered_match_offers_no_ghost() {
        let r = registry(vec![Fake::new("model")]);
        let s = state(&r, "/mdl");
        assert_eq!(labels(&s), vec!["/model"], "it still matches");
        assert_eq!(s.ghost, None, "but `mdl` is not a prefix of `model`");
    }

    #[test]
    fn a_fully_typed_name_has_nothing_left_to_ghost() {
        let r = registry(vec![Fake::new("new")]);
        assert_eq!(state(&r, "/new").ghost, None);
        assert_eq!(state(&r, "/").ghost, None, "nothing typed yet");
    }

    /// The command-name ghost is drawn at the caret, so it is only offered when the
    /// token ends the line — never over text the user can see.
    #[test]
    fn no_ghost_when_text_follows_the_command_token() {
        let r = registry(vec![Fake::new("commit")]);
        let mut s = SlashState::default();
        // `/comm foo` with the caret still inside `comm`.
        s.refresh(
            &r,
            &mut FuzzyMatcher::new(),
            &CommandCtx::default(),
            "/comm foo",
            5,
        );
        assert!(s.open, "the menu is still offering commands");
        assert_eq!(s.ghost, None);
    }

    /// In the argument phase the hint is what the command expects, derived from its
    /// usage line — and it disappears once an argument is typed.
    #[test]
    fn the_argument_hint_shows_what_the_command_expects() {
        let r = registry(vec![Fake::new("model").suggesting(&["fast"])]);
        // `Fake::usage` is "/fake", which has no argument part…
        assert_eq!(state(&r, "/model ").ghost, None);

        // …so use a real builtin, whose usage line does.
        let mut real = CommandRegistry::new();
        crate::commands::register_builtins(&mut real);
        assert_eq!(
            state(&real, "/help ").ghost.as_deref(),
            Some("[command]"),
            "the argument part of `/help [command]`"
        );
        assert_eq!(
            state(&real, "/help n").ghost,
            None,
            "an argument was typed; the hint has done its job"
        );
        assert_eq!(
            state(&real, "/quit ").ghost,
            None,
            "a command taking no arguments hints nothing"
        );
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
