//! App state and the sans-IO reducer: `Msg → update(&mut App, now) → Vec<Cmd>`
//! (grok's dispatch discipline, `src/app/actions.rs:1-8` — "dispatch stays
//! sans-IO"). All interaction semantics live here so they are table-testable
//! without a terminal.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use locode_core::{ContentBlock, Event, Report, ResultChunk, Role};

use crate::approval::{ApprovalOutcome, ApprovalView};
use crate::engine::EngineMsg;
use crate::ui::blocks::{Block, turn_end};
use crate::ui::composer::Composer;

/// The default reason a bare Deny sends (typed feedback is a slice-5 polish).
const DENY_REASON: &str = "denied by user";

/// Double-press window for Esc-clear and the Ctrl+C quit arm (grok uses
/// 800 ms for double-Esc; codex's quit arm is the same order of magnitude).
pub const ARM_WINDOW: Duration = Duration::from_millis(800);

/// Everything the reducer consumes.
#[derive(Debug)]
pub enum Msg {
    /// A terminal event from the input-reader thread (boxed — the
    /// `Paste` variant is large).
    Input(Box<CrosstermEvent>),
    /// A message from the engine task.
    Engine(Box<EngineMsg>),
    /// A tool call is awaiting approval (the loop kept the oneshot).
    Approval(ApprovalView),
    /// Animation tick (sent by the loop only while a run is active).
    Tick,
    /// SIGINT/SIGTERM arrived (graceful quit path).
    SignalQuit,
}

/// Maximum prompt-history entries kept (grok's cap).
const HISTORY_CAP: usize = 200;

/// Everything the reducer asks the loop to do (the loop owns all IO).
#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Forward this prompt to the engine task.
    Submit(String),
    /// Discard the session and build a fresh one (`/new`).
    NewSession,
    /// Fire the current run's cancel handle (the loop holds it — ADR-0018).
    CancelRun,
    /// Resolve a pending approval (the loop holds the oneshot, keyed by id).
    ResolveApproval {
        /// The `tool_use` id whose approval this answers.
        id: String,
        /// The user's choice.
        outcome: ApprovalOutcome,
    },
    /// Tear down and exit.
    Quit,
}

/// A transient one-line hint shown in the footer (quit arming, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// "press ctrl+c again to quit"
    QuitArmed,
    /// "press esc again to clear"
    ClearArmed,
    /// "cancelling…" (Esc/Ctrl+C fired the cancel handle; awaiting the
    /// terminal report).
    Cancelling,
}

/// Whether a run is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// Ready for a prompt.
    Idle,
    /// The engine is driving a run submitted at this instant.
    Running {
        /// When the run started (UI-side clock, for elapsed display).
        started: Instant,
        /// The cancel handle was fired; awaiting the terminal report.
        cancelling: bool,
    },
}

/// An assistant tool call whose result hasn't arrived yet (shown in the
/// status row; the block prints only when the result pairs — print-once).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTool {
    /// The `tool_use` id (pairing key).
    pub id: String,
    /// Client-facing tool name.
    pub name: String,
    /// One-line argument summary.
    pub args: String,
}

/// The whole UI state (one struct owned by the event loop — the ratatui
/// answer to Claude Code's ref-mirror epidemic; study §5).
pub struct App {
    /// The multiline prompt editor.
    pub composer: Composer,
    /// Set when the loop should exit after the current iteration.
    pub should_quit: bool,
    /// Redraw needed.
    pub dirty: bool,
    /// Run lifecycle.
    pub run: RunState,
    /// Finalized blocks awaiting `insert_before` (drained by the loop).
    pub outbox: Vec<Block>,
    /// Tool calls awaiting their results.
    pub pending_tools: Vec<PendingTool>,
    /// Approvals awaiting a user decision — FIFO, only the front renders
    /// (grok's rule). The loop holds the matching oneshots.
    pub approval_queue: VecDeque<ApprovalView>,
    /// The composer draft stashed while an approval overlay is up (restored
    /// when the queue empties — grok's flow).
    stashed_draft: Option<String>,
    /// Prompts submitted while a run was active — drained one per turn end
    /// (codex's queue-and-drain).
    pub prompt_queue: VecDeque<String>,
    /// Prompt history, most-recent-first (move-to-front dedup, cap 200).
    history: Vec<String>,
    /// History browse cursor (`None` = not browsing); index into `history`.
    history_nav: Option<usize>,
    /// The live draft saved when history browsing began (restored on exit).
    history_saved: Option<String>,
    /// Resolved model id (footer display); `None` until the engine is ready.
    pub model: Option<String>,
    /// Session assembly failed — submits are disabled.
    pub engine_failed: bool,
    /// Spinner frame counter (advanced by `Msg::Tick`).
    pub spinner_frame: usize,
    /// Ctrl+C quit arm: armed until this instant.
    quit_armed_until: Option<Instant>,
    /// Esc clear-draft arm: armed until this instant.
    esc_armed_until: Option<Instant>,
    /// The active footer hint, if any.
    pub hint: Option<Hint>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Fresh state with an empty composer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            composer: Composer::new(),
            should_quit: false,
            dirty: true,
            run: RunState::Idle,
            outbox: Vec::new(),
            pending_tools: Vec::new(),
            approval_queue: VecDeque::new(),
            stashed_draft: None,
            prompt_queue: VecDeque::new(),
            history: Vec::new(),
            history_nav: None,
            history_saved: None,
            model: None,
            engine_failed: false,
            spinner_frame: 0,
            quit_armed_until: None,
            esc_armed_until: None,
            hint: None,
        }
    }

    /// Whether a run is currently active.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.run, RunState::Running { .. })
    }

    /// Whether an approval overlay is up (owns the input).
    #[must_use]
    pub fn is_awaiting_approval(&self) -> bool {
        !self.approval_queue.is_empty()
    }

    /// The reducer. Pure over (`self`, `msg`, `now`): no IO, no clock reads —
    /// the loop injects `now` so tests control time.
    pub fn update(&mut self, msg: Msg, now: Instant) -> Vec<Cmd> {
        self.dirty = true;
        match msg {
            Msg::SignalQuit => {
                self.should_quit = true;
                vec![Cmd::Quit]
            }
            Msg::Tick => {
                self.spinner_frame = self.spinner_frame.wrapping_add(1);
                vec![]
            }
            Msg::Engine(engine_msg) => self.on_engine(*engine_msg, now),
            Msg::Approval(view) => {
                self.on_approval(view);
                vec![]
            }
            Msg::Input(event) => match *event {
                CrosstermEvent::Key(key) => self.on_key(key, now),
                CrosstermEvent::Paste(text) => {
                    // Normalize CR pastes (Windows/legacy terminals) to LF.
                    self.composer
                        .insert_text(&text.replace("\r\n", "\n").replace('\r', "\n"));
                    vec![]
                }
                CrosstermEvent::Resize(..) => vec![], // redraw via dirty
                _ => {
                    self.dirty = false; // focus/mouse events: nothing to do
                    vec![]
                }
            },
        }
    }

    fn on_engine(&mut self, msg: EngineMsg, now: Instant) -> Vec<Cmd> {
        match msg {
            EngineMsg::Ready { model } => {
                self.model = Some(model);
                vec![]
            }
            EngineMsg::BuildFailed(message) => {
                self.engine_failed = true;
                self.outbox.push(Block::Notice(format!(
                    "engine unavailable: {message} (ctrl+c to quit)"
                )));
                vec![]
            }
            // The loop captures the cancel handle; the reducer only tracks
            // that a run is active (sans-IO — no token stored here).
            EngineMsg::RunStarted { .. } => {
                self.run = RunState::Running {
                    started: now,
                    cancelling: false,
                };
                self.hint = None;
                vec![]
            }
            EngineMsg::Event(event) => {
                self.on_event(*event);
                vec![]
            }
            // The loop takes the responder before forwarding the view; the
            // reducer never sees `Approval` on the Engine channel.
            EngineMsg::Approval(_) => vec![],
            EngineMsg::RunFinished(report) => self.on_run_finished(&report, now),
            EngineMsg::SessionReset => {
                self.run = RunState::Idle;
                self.pending_tools.clear();
                self.approval_queue.clear();
                self.prompt_queue.clear();
                self.outbox.push(Block::Notice("— new session —".into()));
                vec![]
            }
        }
    }

    /// Enqueue an approval; stash the draft on the empty→non-empty transition
    /// so the composer is free for follow-up (grok's flow).
    fn on_approval(&mut self, view: ApprovalView) {
        if self.approval_queue.is_empty() {
            self.stashed_draft = Some(self.composer.take_text());
        }
        self.approval_queue.push_back(view);
    }

    /// Translate one engine event into transcript state (SPEC-TUI mapping).
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Message { message } => match message.role {
                Role::Assistant => {
                    for block in message.content {
                        match block {
                            ContentBlock::Text { text } => {
                                self.outbox.push(Block::AssistantText(text));
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                self.pending_tools.push(PendingTool {
                                    id,
                                    name,
                                    args: args_summary(&input),
                                });
                            }
                            _ => {} // thinking/other: not rendered in v1
                        }
                    }
                }
                Role::User => {
                    // Tool results pair with pending calls; plain user text
                    // was already echoed by the UI at submit time.
                    for block in message.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
                            self.finalize_tool(&tool_use_id, &content, is_error);
                        }
                    }
                }
                _ => {}
            },
            Event::Error { message } => self.outbox.push(Block::Notice(message)),
            // Init is chrome-irrelevant here; Approval lands in slice 4;
            // Result rides EngineMsg::RunFinished.
            _ => {}
        }
    }

    fn finalize_tool(&mut self, tool_use_id: &str, content: &[ResultChunk], is_error: bool) {
        let body = content
            .iter()
            .map(|chunk| match chunk {
                ResultChunk::Text { text } => text.as_str(),
                ResultChunk::Image { .. } => "[image]",
            })
            .collect::<Vec<_>>()
            .join("\n");
        let (name, args) = match self.pending_tools.iter().position(|p| p.id == tool_use_id) {
            Some(i) => {
                let p = self.pending_tools.remove(i);
                (p.name, p.args)
            }
            None => ("tool".to_string(), String::new()),
        };
        self.outbox.push(Block::ToolCall {
            name,
            args,
            is_error,
            body,
        });
    }

    fn on_run_finished(&mut self, report: &Report, now: Instant) -> Vec<Cmd> {
        // Defensive: pairing guarantees results for every tool_use, but a
        // future terminal path must never strand a pending entry silently.
        for p in std::mem::take(&mut self.pending_tools) {
            self.outbox.push(Block::ToolCall {
                name: p.name,
                args: p.args,
                is_error: true,
                body: "(no result)".into(),
            });
        }
        let elapsed = match self.run {
            RunState::Running { started, .. } => now.duration_since(started).as_secs(),
            RunState::Idle => 0,
        };
        self.outbox.push(turn_end(report, elapsed));
        self.run = RunState::Idle;
        // Defensive: a terminal report clears any lingering overlay (the loop
        // drains the matching oneshots). The queue should already be empty.
        if !self.approval_queue.is_empty() {
            self.approval_queue.clear();
            self.restore_draft();
        }
        // Drain one queued prompt per completion (codex's cadence).
        self.drain_queued_prompt()
    }

    /// Pop and submit the next queued prompt, if any (called at turn end).
    fn drain_queued_prompt(&mut self) -> Vec<Cmd> {
        match self.prompt_queue.pop_front() {
            Some(text) => {
                self.outbox.push(Block::UserPrompt(text.clone()));
                vec![Cmd::Submit(text)]
            }
            None => vec![],
        }
    }

    /// Resolve the front approval with `outcome`, pop it, and restore the
    /// stashed draft once the queue empties.
    fn resolve_front_approval(&mut self, outcome: ApprovalOutcome) -> Vec<Cmd> {
        let Some(view) = self.approval_queue.pop_front() else {
            return vec![];
        };
        if self.approval_queue.is_empty() {
            self.restore_draft();
        }
        vec![Cmd::ResolveApproval {
            id: view.tool_use_id,
            outcome,
        }]
    }

    fn restore_draft(&mut self) {
        if let Some(draft) = self.stashed_draft.take() {
            self.composer.set_text(&draft);
        }
    }

    /// Overlay key handling while an approval is pending (non-Ctrl keys).
    /// y/Enter = allow, a = allow-for-session, d/Esc = deny.
    fn on_approval_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.resolve_front_approval(ApprovalOutcome::Allow)
            }
            KeyCode::Char('a') => self.resolve_front_approval(ApprovalOutcome::AllowSession),
            KeyCode::Char('d') | KeyCode::Esc => {
                self.resolve_front_approval(ApprovalOutcome::Deny {
                    reason: DENY_REASON.to_string(),
                })
            }
            _ => vec![], // ignore other keys while the overlay owns input
        }
    }

    fn on_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // The approval overlay owns non-Ctrl input; Ctrl+C/Ctrl+D still fall
        // through to cancel/quit (cancel drains the queue).
        if self.is_awaiting_approval() && !ctrl {
            return self.on_approval_key(key);
        }
        match (key.code, ctrl) {
            // Ctrl+C: (spec) first press cancels a running turn AND arms the
            // quit hint; a second press within the window quits. At idle:
            // clear a non-empty draft first (grok's two-step,
            // `agent_view/mod.rs:22-26`), else arm-then-quit (codex's
            // arm/confirm, `interaction.rs:360-414`).
            (KeyCode::Char('c'), true) => {
                if Self::is_armed(self.quit_armed_until, now) {
                    self.should_quit = true;
                    return vec![Cmd::Quit];
                }
                let mut cmds = Vec::new();
                if self.is_running() {
                    cmds.push(self.begin_cancel());
                } else if !self.composer.is_empty() {
                    // Idle with a draft: clear it, don't arm quit.
                    self.composer.clear();
                    self.disarm();
                    return vec![];
                }
                self.quit_armed_until = Some(now + ARM_WINDOW);
                if !self.is_running() {
                    self.hint = Some(Hint::QuitArmed);
                }
                cmds
            }
            // Ctrl+D: quit only on an empty composer (codex,
            // `interaction.rs:420-445`); otherwise ignored in v1.
            (KeyCode::Char('d'), true) => {
                if self.composer.is_empty() {
                    self.should_quit = true;
                    vec![Cmd::Quit]
                } else {
                    vec![]
                }
            }
            // Esc while running: cancel the turn (spec) — first press,
            // idempotent re-fire on a stuck run (grok's retry rule,
            // `dispatch/turn.rs:68-95`). Esc at idle: pop the last queued
            // prompt back into the composer, else double-press clears a
            // non-empty draft (grok's 800 ms TTL).
            (KeyCode::Esc, _) => {
                if self.is_running() {
                    return vec![self.begin_cancel()];
                }
                if let Some(text) = self.prompt_queue.pop_back() {
                    // Un-queue the most recently queued prompt (codex's
                    // edit-queued gesture, mapped to Esc per our spec).
                    self.composer.set_text(&text);
                    self.disarm();
                    return vec![];
                }
                if self.composer.is_empty() {
                    self.disarm();
                    return vec![];
                }
                if Self::is_armed(self.esc_armed_until, now) {
                    self.composer.clear();
                    self.disarm();
                    return vec![];
                }
                self.esc_armed_until = Some(now + ARM_WINDOW);
                self.hint = Some(Hint::ClearArmed);
                vec![]
            }
            // Up/Down browse prompt history (gated to single-line/empty so a
            // multiline draft is never clobbered).
            (KeyCode::Up, false) if self.can_history_nav() => {
                self.history_prev();
                vec![]
            }
            (KeyCode::Down, false) if self.history_nav.is_some() => {
                self.history_next();
                vec![]
            }
            // Enter submits; Alt+Enter inserts a newline (works without the
            // kitty protocol — deferred).
            (KeyCode::Enter, _) => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.insert_newline();
                    return vec![];
                }
                let text = self.composer.take_text();
                self.disarm();
                self.history_nav = None;
                if text.trim().is_empty() {
                    return vec![];
                }
                // Slash commands intercept before submit/queue.
                if let Some(cmds) = self.try_slash(&text) {
                    return cmds;
                }
                if self.engine_failed || self.model.is_none() {
                    self.composer.set_text(&text); // engine not ready; keep it
                    return vec![];
                }
                self.record_history(&text);
                // Running ⇒ queue (drained one per turn end); else submit.
                if self.is_running() {
                    self.prompt_queue.push_back(text);
                    return vec![];
                }
                self.outbox.push(Block::UserPrompt(text.clone()));
                vec![Cmd::Submit(text)]
            }
            // Everything else goes to the editor; any keypress disarms the
            // pending quit/clear arms and exits history browsing.
            _ => {
                self.disarm();
                self.history_nav = None;
                self.composer.input(key);
                vec![]
            }
        }
    }

    /// Slash-command dispatch (`/quit`, `/new`); `None` if `text` isn't a
    /// recognized command shape (falls through to submit/queue).
    fn try_slash(&mut self, text: &str) -> Option<Vec<Cmd>> {
        let trimmed = text.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        match trimmed {
            "/quit" | "/exit" => {
                self.should_quit = true;
                Some(vec![Cmd::Quit])
            }
            "/new" => {
                if self.is_running() {
                    self.outbox
                        .push(Block::Notice("finish or cancel the run before /new".into()));
                    Some(vec![])
                } else {
                    Some(vec![Cmd::NewSession])
                }
            }
            other => {
                self.outbox
                    .push(Block::Notice(format!("unknown command: {other}")));
                Some(vec![])
            }
        }
    }

    /// Record a submitted prompt in history (move-to-front dedup, cap).
    fn record_history(&mut self, text: &str) {
        self.history.retain(|h| h != text);
        self.history.insert(0, text.to_owned());
        self.history.truncate(HISTORY_CAP);
    }

    /// History nav is allowed only from an empty/single-line composer, or
    /// while already browsing (so a multiline draft is never clobbered).
    fn can_history_nav(&self) -> bool {
        !self.history.is_empty()
            && (self.history_nav.is_some() || !self.composer.text().contains('\n'))
    }

    fn history_prev(&mut self) {
        let next = match self.history_nav {
            None => {
                self.history_saved = Some(self.composer.text());
                0
            }
            Some(i) => (i + 1).min(self.history.len() - 1),
        };
        self.history_nav = Some(next);
        let entry = self.history[next].clone();
        self.composer.set_text(&entry);
    }

    fn history_next(&mut self) {
        match self.history_nav {
            Some(0) | None => {
                // Back to the live draft.
                let draft = self.history_saved.take().unwrap_or_default();
                self.composer.set_text(&draft);
                self.history_nav = None;
            }
            Some(i) => {
                let idx = i - 1;
                self.history_nav = Some(idx);
                let entry = self.history[idx].clone();
                self.composer.set_text(&entry);
            }
        }
    }

    /// Mark the current run as cancelling and ask the loop to fire the handle
    /// (idempotent — safe to call repeatedly on a stuck run).
    fn begin_cancel(&mut self) -> Cmd {
        if let RunState::Running { cancelling, .. } = &mut self.run {
            *cancelling = true;
        }
        self.hint = Some(Hint::Cancelling);
        Cmd::CancelRun
    }

    fn is_armed(armed_until: Option<Instant>, now: Instant) -> bool {
        armed_until.is_some_and(|until| now <= until)
    }

    fn disarm(&mut self) {
        self.quit_armed_until = None;
        self.esc_armed_until = None;
        self.hint = None;
    }
}

/// One-line JSON argument summary for tool status/blocks, capped for the
/// status row.
fn args_summary(input: &serde_json::Value) -> String {
    let mut s = input.to_string();
    if s.chars().count() > 60 {
        s = s.chars().take(59).collect::<String>() + "…";
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use locode_core::{Message, Status, ToolCallRecord, Usage};
    use serde_json::json;

    fn key(code: KeyCode) -> Msg {
        Msg::Input(Box::new(CrosstermEvent::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        ))))
    }
    fn ctrl(c: char) -> Msg {
        Msg::Input(Box::new(CrosstermEvent::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        ))))
    }
    fn alt_enter() -> Msg {
        Msg::Input(Box::new(CrosstermEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::ALT,
        ))))
    }
    fn run_started() -> Msg {
        Msg::Engine(Box::new(EngineMsg::RunStarted {
            cancel: locode_core::CancellationToken::new(),
        }))
    }
    fn type_str(app: &mut App, s: &str, now: Instant) {
        for ch in s.chars() {
            let _ = app.update(key(KeyCode::Char(ch)), now);
        }
    }
    fn ready_app() -> App {
        let mut app = App::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Ready {
                model: "mock-1".into(),
            })),
            Instant::now(),
        );
        app
    }
    fn report(status: Status) -> Report {
        Report {
            schema_version: 1,
            status,
            harness: "grok".into(),
            api_schema: "mock".into(),
            final_message: None,
            structured_output: None,
            turns: 2,
            tool_calls: Vec::<ToolCallRecord>::new(),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 20,
                ..Usage::default()
            },
            session_id: "s".into(),
            stop_reason: None,
            error: None,
        }
    }

    // ---- slice 1 interaction contract (unchanged semantics) ----

    #[test]
    fn ctrl_c_clears_draft_then_arms_then_quits() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "draft", t0);
        assert_eq!(app.update(ctrl('c'), t0), vec![]);
        assert!(app.composer.is_empty());
        assert_eq!(app.update(ctrl('c'), t0), vec![]);
        assert_eq!(app.hint, Some(Hint::QuitArmed));
        assert_eq!(
            app.update(ctrl('c'), t0 + Duration::from_millis(300)),
            vec![Cmd::Quit]
        );
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_arm_expires() {
        let mut app = ready_app();
        let t0 = Instant::now();
        assert_eq!(app.update(ctrl('c'), t0), vec![]);
        assert_eq!(
            app.update(ctrl('c'), t0 + ARM_WINDOW + Duration::from_millis(1)),
            vec![]
        );
        assert!(!app.should_quit);
        assert_eq!(app.hint, Some(Hint::QuitArmed));
    }

    #[test]
    fn ctrl_d_quits_only_on_empty_composer() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "x", t0);
        assert_eq!(app.update(ctrl('d'), t0), vec![]);
        assert!(!app.should_quit);
        app.composer.clear();
        assert_eq!(app.update(ctrl('d'), t0), vec![Cmd::Quit]);
    }

    #[test]
    fn esc_double_press_clears_draft_within_window() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "keep me", t0);
        assert_eq!(app.update(key(KeyCode::Esc), t0), vec![]);
        assert!(!app.composer.is_empty());
        assert_eq!(
            app.update(key(KeyCode::Esc), t0 + Duration::from_millis(500)),
            vec![]
        );
        assert!(app.composer.is_empty());
    }

    #[test]
    fn esc_arm_expires_and_typing_disarms() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "draft", t0);
        let _ = app.update(key(KeyCode::Esc), t0);
        let late = t0 + ARM_WINDOW + Duration::from_millis(1);
        let _ = app.update(key(KeyCode::Esc), late);
        assert!(!app.composer.is_empty());
        let _ = app.update(key(KeyCode::Char('!')), late);
        assert_eq!(app.hint, None);
    }

    #[test]
    fn enter_submits_and_alt_enter_inserts_newline() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "line one", t0);
        let _ = app.update(alt_enter(), t0);
        type_str(&mut app, "line two", t0);
        let cmds = app.update(key(KeyCode::Enter), t0);
        assert_eq!(cmds, vec![Cmd::Submit("line one\nline two".into())]);
        assert!(app.composer.is_empty());
        assert_eq!(
            app.outbox,
            vec![Block::UserPrompt("line one\nline two".into())],
            "submit echoes the prompt block"
        );
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
    }

    #[test]
    fn paste_normalizes_carriage_returns() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(
            Msg::Input(Box::new(CrosstermEvent::Paste("a\r\nb\rc".into()))),
            t0,
        );
        let cmds = app.update(key(KeyCode::Enter), t0);
        assert_eq!(cmds, vec![Cmd::Submit("a\nb\nc".into())]);
    }

    #[test]
    fn signal_quit_is_immediate() {
        let mut app = App::new();
        assert_eq!(app.update(Msg::SignalQuit, Instant::now()), vec![Cmd::Quit]);
    }

    // ---- slice 2: run lifecycle + event translation ----

    #[test]
    fn submit_requires_a_ready_engine() {
        let t0 = Instant::now();
        // Not ready: Enter keeps the draft, no command.
        let mut app = App::new();
        type_str(&mut app, "hi", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
        assert!(!app.composer.is_empty());
    }

    #[test]
    fn assistant_events_become_blocks_and_pending_tools() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::Message {
                message: Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Text {
                            text: "working on it".into(),
                        },
                        ContentBlock::ToolUse {
                            id: "c1".into(),
                            name: "run_terminal_cmd".into(),
                            input: json!({"command": "ls"}),
                        },
                    ],
                },
            })))),
            t0,
        );
        assert_eq!(
            app.outbox,
            vec![Block::AssistantText("working on it".into())]
        );
        assert_eq!(app.pending_tools.len(), 1);
        assert_eq!(app.pending_tools[0].name, "run_terminal_cmd");

        // Result pairs and finalizes the tool block, error flag preserved.
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::Message {
                message: Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "c1".into(),
                        content: vec![ResultChunk::Text {
                            text: "exit: 0".into(),
                        }],
                        is_error: false,
                    }],
                },
            })))),
            t0,
        );
        assert!(app.pending_tools.is_empty());
        assert!(matches!(
            app.outbox.last(),
            Some(Block::ToolCall { name, is_error: false, .. }) if name == "run_terminal_cmd"
        ));
    }

    #[test]
    fn plain_user_messages_are_skipped_not_duplicated() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::Message {
                message: Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "<user_query>hi</user_query>".into(),
                    }],
                },
            })))),
            t0,
        );
        assert!(app.outbox.is_empty(), "wrapped echo must not render");
    }

    #[test]
    fn run_finished_flushes_pending_and_appends_separator() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        app.pending_tools.push(PendingTool {
            id: "c9".into(),
            name: "grep".into(),
            args: "{}".into(),
        });
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0 + Duration::from_secs(41),
        );
        assert!(matches!(app.run, RunState::Idle));
        assert!(matches!(
            &app.outbox[0],
            Block::ToolCall { is_error: true, body, .. } if body == "(no result)"
        ));
        assert!(matches!(
            &app.outbox[1],
            Block::TurnEnd {
                status: Status::Completed,
                turns: 2,
                tokens: 120,
                elapsed_secs: 41
            }
        ));
    }

    #[test]
    fn build_failure_disables_submits_with_notice() {
        let mut app = App::new();
        let t0 = Instant::now();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::BuildFailed("no key".into()))),
            t0,
        );
        assert!(app.engine_failed);
        assert!(matches!(&app.outbox[0], Block::Notice(n) if n.contains("no key")));
        type_str(&mut app, "hi", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
    }

    // ---- slice 3: cancel ----

    #[test]
    fn esc_while_running_cancels_idempotently() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);

        // First Esc cancels; state shows cancelling.
        assert_eq!(app.update(key(KeyCode::Esc), t0), vec![Cmd::CancelRun]);
        assert!(matches!(
            app.run,
            RunState::Running {
                cancelling: true,
                ..
            }
        ));
        assert_eq!(app.hint, Some(Hint::Cancelling));

        // Second Esc re-fires (idempotent retry on a stuck run).
        assert_eq!(app.update(key(KeyCode::Esc), t0), vec![Cmd::CancelRun]);
    }

    #[test]
    fn esc_at_idle_still_clears_draft_not_cancel() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "draft", t0);
        // Not running: Esc arms clear, no CancelRun.
        assert_eq!(app.update(key(KeyCode::Esc), t0), vec![]);
        assert_eq!(app.hint, Some(Hint::ClearArmed));
    }

    #[test]
    fn ctrl_c_while_running_cancels_and_arms_quit() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);

        // First Ctrl+C: cancel the run AND arm quit (spec).
        assert_eq!(app.update(ctrl('c'), t0), vec![Cmd::CancelRun]);
        assert!(matches!(
            app.run,
            RunState::Running {
                cancelling: true,
                ..
            }
        ));

        // Second Ctrl+C within the window: quit.
        assert_eq!(
            app.update(ctrl('c'), t0 + Duration::from_millis(200)),
            vec![Cmd::Quit]
        );
        assert!(app.should_quit);
    }

    #[test]
    fn cancelled_run_settles_to_idle_with_cancelled_separator() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let _ = app.update(key(KeyCode::Esc), t0);

        // The engine settles the run with a Cancelled report (ADR-0018).
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Cancelled,
            ))))),
            t0 + Duration::from_secs(3),
        );
        assert!(
            matches!(app.run, RunState::Idle),
            "settles only on the report"
        );
        assert!(matches!(
            app.outbox.last(),
            Some(Block::TurnEnd {
                status: Status::Cancelled,
                ..
            })
        ));
    }

    // ---- slice 4: approvals ----

    fn approval_view(id: &str, name: &str) -> ApprovalView {
        ApprovalView {
            tool_use_id: id.into(),
            tool_name: name.into(),
            kind: "shell".into(),
            args: "{}".into(),
        }
    }

    #[test]
    fn approval_enqueues_stashes_draft_and_allow_resolves_and_restores() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        type_str(&mut app, "my draft", t0);

        // Ask arrives → queued, draft stashed, composer cleared.
        let _ = app.update(Msg::Approval(approval_view("c1", "run_terminal_cmd")), t0);
        assert!(app.is_awaiting_approval());
        assert!(app.composer.is_empty(), "draft stashed while overlay is up");

        // `y` allows → resolves the front and restores the draft.
        let cmds = app.update(key(KeyCode::Char('y')), t0);
        assert_eq!(
            cmds,
            vec![Cmd::ResolveApproval {
                id: "c1".into(),
                outcome: ApprovalOutcome::Allow,
            }]
        );
        assert!(!app.is_awaiting_approval());
        assert_eq!(app.composer.text(), "my draft", "draft restored");
    }

    #[test]
    fn approval_deny_and_allow_session_map_correctly() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);

        let _ = app.update(Msg::Approval(approval_view("c1", "grep")), t0);
        assert_eq!(
            app.update(key(KeyCode::Char('d')), t0),
            vec![Cmd::ResolveApproval {
                id: "c1".into(),
                outcome: ApprovalOutcome::Deny {
                    reason: "denied by user".into()
                },
            }]
        );

        let _ = app.update(Msg::Approval(approval_view("c2", "grep")), t0);
        assert_eq!(
            app.update(key(KeyCode::Char('a')), t0),
            vec![Cmd::ResolveApproval {
                id: "c2".into(),
                outcome: ApprovalOutcome::AllowSession,
            }]
        );

        // Esc denies too.
        let _ = app.update(Msg::Approval(approval_view("c3", "grep")), t0);
        assert!(matches!(
            app.update(key(KeyCode::Esc), t0).as_slice(),
            [Cmd::ResolveApproval {
                outcome: ApprovalOutcome::Deny { .. },
                ..
            }]
        ));
    }

    #[test]
    fn ctrl_c_still_cancels_while_an_approval_pends() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let _ = app.update(Msg::Approval(approval_view("c1", "grep")), t0);
        // Ctrl+C falls through to cancel (the loop then drains the queue).
        assert_eq!(app.update(ctrl('c'), t0), vec![Cmd::CancelRun]);
    }

    #[test]
    fn run_finished_clears_a_lingering_overlay() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let _ = app.update(Msg::Approval(approval_view("c1", "grep")), t0);
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Cancelled,
            ))))),
            t0,
        );
        assert!(!app.is_awaiting_approval());
    }

    // ---- slice 5a: queued prompts, history, slash ----

    #[test]
    fn enter_while_running_queues_and_turn_end_drains_one() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        type_str(&mut app, "next prompt", t0);
        // Queued, not dropped, no command.
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
        assert_eq!(app.prompt_queue.len(), 1);
        assert!(app.composer.is_empty());

        // Second queued waits behind the first.
        type_str(&mut app, "and another", t0);
        let _ = app.update(key(KeyCode::Enter), t0);
        assert_eq!(app.prompt_queue.len(), 2);

        // Turn end drains exactly one (echoed + submitted).
        let cmds = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        assert_eq!(cmds, vec![Cmd::Submit("next prompt".into())]);
        assert_eq!(app.prompt_queue.len(), 1);
        assert!(matches!(app.outbox.last(), Some(Block::UserPrompt(p)) if p == "next prompt"));
    }

    #[test]
    fn esc_at_idle_pops_the_last_queued_prompt() {
        let mut app = ready_app();
        let t0 = Instant::now();
        app.prompt_queue.push_back("first".into());
        app.prompt_queue.push_back("second".into());
        let _ = app.update(key(KeyCode::Esc), t0);
        assert_eq!(app.composer.text(), "second", "last queued popped back");
        assert_eq!(app.prompt_queue.len(), 1);
    }

    #[test]
    fn prompt_history_records_dedups_and_navigates() {
        let mut app = ready_app();
        let t0 = Instant::now();
        // Submit two prompts (records history, most-recent-first).
        type_str(&mut app, "one", t0);
        let _ = app.update(key(KeyCode::Enter), t0);
        type_str(&mut app, "two", t0);
        let _ = app.update(key(KeyCode::Enter), t0);
        // Re-submit "one" → move-to-front dedup (no duplicate).
        type_str(&mut app, "one", t0);
        let _ = app.update(key(KeyCode::Enter), t0);
        assert_eq!(app.history, vec!["one", "two"]);

        // Up recalls most-recent, Up again older, Down restores.
        let _ = app.update(key(KeyCode::Up), t0);
        assert_eq!(app.composer.text(), "one");
        let _ = app.update(key(KeyCode::Up), t0);
        assert_eq!(app.composer.text(), "two");
        let _ = app.update(key(KeyCode::Down), t0);
        assert_eq!(app.composer.text(), "one");
        let _ = app.update(key(KeyCode::Down), t0);
        assert_eq!(app.composer.text(), "", "back to the (empty) live draft");
    }

    #[test]
    fn history_nav_disabled_with_a_multiline_draft() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "past", t0);
        let _ = app.update(key(KeyCode::Enter), t0); // history: ["past"]
        // Build a multiline draft; Up must go to the editor, not history.
        type_str(&mut app, "line one", t0);
        let _ = app.update(alt_enter(), t0);
        type_str(&mut app, "line two", t0);
        let _ = app.update(key(KeyCode::Up), t0);
        assert!(app.composer.text().contains("line one"), "draft preserved");
        assert!(app.history_nav.is_none());
    }

    #[test]
    fn slash_quit_and_new_and_unknown() {
        let mut app = ready_app();
        let t0 = Instant::now();

        type_str(&mut app, "/quit", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![Cmd::Quit]);
        assert!(app.should_quit);

        let mut app = ready_app();
        type_str(&mut app, "/new", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![Cmd::NewSession]);

        // Unknown slash → notice, no command, not submitted.
        type_str(&mut app, "/bogus", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
        assert!(
            matches!(app.outbox.last(), Some(Block::Notice(n)) if n.contains("unknown command"))
        );

        // /new while running → notice, no reset.
        let _ = app.update(run_started(), t0);
        type_str(&mut app, "/new", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
        assert!(
            matches!(app.outbox.last(), Some(Block::Notice(n)) if n.contains("cancel the run"))
        );
    }

    #[test]
    fn session_reset_clears_transcript_state() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        app.prompt_queue.push_back("q".into());
        app.pending_tools.push(PendingTool {
            id: "c".into(),
            name: "grep".into(),
            args: "{}".into(),
        });
        let _ = app.update(Msg::Engine(Box::new(EngineMsg::SessionReset)), t0);
        assert!(matches!(app.run, RunState::Idle));
        assert!(app.prompt_queue.is_empty());
        assert!(app.pending_tools.is_empty());
        assert!(matches!(app.outbox.last(), Some(Block::Notice(n)) if n.contains("new session")));
    }
}
