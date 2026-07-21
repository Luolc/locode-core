//! App state and the sans-IO reducer: `Msg → update(&mut App, now) → Vec<Cmd>`
//! (grok's dispatch discipline, `src/app/actions.rs:1-8` — "dispatch stays
//! sans-IO"). All interaction semantics live here so they are table-testable
//! without a terminal.

use std::time::{Duration, Instant};

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use locode_core::{ContentBlock, Event, Report, ResultChunk, Role};

use crate::engine::EngineMsg;
use crate::ui::blocks::{Block, turn_end};
use crate::ui::composer::Composer;

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
    /// Animation tick (sent by the loop only while a run is active).
    Tick,
    /// SIGINT/SIGTERM arrived (graceful quit path).
    SignalQuit,
}

/// Everything the reducer asks the loop to do (the loop owns all IO).
#[derive(Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Forward this prompt to the engine task.
    Submit(String),
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
    /// "run in progress" (Enter while running; queueing is slice 5)
    RunInProgress,
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
            Msg::Engine(engine_msg) => {
                self.on_engine(*engine_msg, now);
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

    fn on_engine(&mut self, msg: EngineMsg, now: Instant) {
        match msg {
            EngineMsg::Ready { model } => {
                self.model = Some(model);
            }
            EngineMsg::BuildFailed(message) => {
                self.engine_failed = true;
                self.outbox.push(Block::Notice(format!(
                    "engine unavailable: {message} (ctrl+c to quit)"
                )));
            }
            EngineMsg::RunStarted => {
                self.run = RunState::Running { started: now };
                self.hint = None;
            }
            EngineMsg::Event(event) => self.on_event(*event),
            EngineMsg::RunFinished(report) => self.on_run_finished(&report, now),
        }
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

    fn on_run_finished(&mut self, report: &Report, now: Instant) {
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
            RunState::Running { started } => now.duration_since(started).as_secs(),
            RunState::Idle => 0,
        };
        self.outbox.push(turn_end(report, elapsed));
        self.run = RunState::Idle;
    }

    fn on_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            // Ctrl+C: clear a non-empty draft first (grok's two-step,
            // `agent_view/mod.rs:22-26`), else arm-then-quit (codex's
            // arm/confirm, `interaction.rs:360-414`).
            (KeyCode::Char('c'), true) => {
                if !self.composer.is_empty() {
                    self.composer.clear();
                    self.disarm();
                    return vec![];
                }
                if Self::is_armed(self.quit_armed_until, now) {
                    self.should_quit = true;
                    return vec![Cmd::Quit];
                }
                self.quit_armed_until = Some(now + ARM_WINDOW);
                self.hint = Some(Hint::QuitArmed);
                vec![]
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
            // Esc at idle: double-press clears a non-empty draft (grok's
            // 800 ms TTL, `agent_view/prompt.rs:751-830`). Cancel-run Esc
            // lands in slice 3.
            (KeyCode::Esc, _) => {
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
            // Enter submits; Alt+Enter inserts a newline (works without the
            // kitty protocol — deferred).
            (KeyCode::Enter, _) => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    self.composer.insert_newline();
                    return vec![];
                }
                if self.is_running() {
                    self.hint = Some(Hint::RunInProgress);
                    return vec![]; // queueing lands in slice 5; draft kept
                }
                if self.engine_failed || self.model.is_none() {
                    return vec![]; // engine not ready; draft kept
                }
                let text = self.composer.take_text();
                self.disarm();
                if text.trim().is_empty() {
                    return vec![];
                }
                self.outbox.push(Block::UserPrompt(text.clone()));
                vec![Cmd::Submit(text)]
            }
            // Everything else goes to the editor; any keypress disarms the
            // pending quit/clear arms.
            _ => {
                self.disarm();
                self.composer.input(key);
                vec![]
            }
        }
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
    fn submit_requires_ready_engine_and_idle_run() {
        let t0 = Instant::now();
        // Not ready: Enter keeps the draft, no command.
        let mut app = App::new();
        type_str(&mut app, "hi", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
        assert!(!app.composer.is_empty());

        // Running: Enter keeps the draft, hint shown.
        let mut app = ready_app();
        let _ = app.update(Msg::Engine(Box::new(EngineMsg::RunStarted)), t0);
        type_str(&mut app, "queued later", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);
        assert_eq!(app.hint, Some(Hint::RunInProgress));
        assert!(!app.composer.is_empty());
    }

    #[test]
    fn assistant_events_become_blocks_and_pending_tools() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(Msg::Engine(Box::new(EngineMsg::RunStarted)), t0);
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
        let _ = app.update(Msg::Engine(Box::new(EngineMsg::RunStarted)), t0);
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
}
