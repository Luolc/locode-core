//! App state and the sans-IO reducer: `Msg → update(&mut App, now) → Vec<Cmd>`
//! (grok's dispatch discipline, `src/app/actions.rs:1-8` — "dispatch stays
//! sans-IO"). All interaction semantics live here so they are table-testable
//! without a terminal.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use locode_core::{ContentBlock, Event, Report, ResultChunk, Role};

use crate::approval::{ApprovalOutcome, ApprovalView};
use crate::commands::{
    CommandCtx, CommandRegistry, CommandResult, FuzzyMatcher, SlashState, UiAction,
    parse_invocation, register_builtins, register_skills,
};
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
    /// Switch the running session's model, and persist it as the next session's
    /// default (`/model <id>`).
    SetModel(String),
    /// Set the running session's effort rung, and persist it (`/effort <rung>`).
    SetEffort(Option<locode_core::Effort>),
    /// Add a working directory to the running session (`/add-dir <path>`).
    AddDir(std::path::PathBuf),
    /// Resolve and run this `/name args` line, then feed the result back through
    /// [`App::apply_command_result`].
    ///
    /// The reducer cannot run it itself: `execute` is async and a skill-backed command
    /// reads its `SKILL.md` from disk, so execution belongs to the loop that owns IO.
    RunCommand {
        /// The whole line, leading slash included.
        line: String,
    },
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

/// A prompt waiting for the current run to finish.
///
/// `display` and `wire` differ for a skill invocation: the transcript shows
/// `/commit fix the typo` while the model receives the whole skill body (grok's
/// `QueuedPrompt { wire_blocks, display }`). For an ordinary prompt they are the same
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedPrompt {
    /// What the queue preview and the transcript show.
    pub display: String,
    /// What the engine receives.
    pub wire: String,
}

impl QueuedPrompt {
    /// A prompt whose display text *is* what the model sees.
    fn plain(text: String) -> Self {
        Self {
            display: text.clone(),
            wire: text,
        }
    }
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
#[allow(clippy::struct_excessive_bools)] // independent UI flags, not a state enum
pub struct App {
    /// The multiline prompt editor.
    pub composer: Composer,
    /// Every registered slash command (ADR-0026). Builtins register at startup;
    /// skill-backed commands join when the engine reports what it discovered.
    pub registry: CommandRegistry,
    /// The command menu derived from the composer on every edit.
    pub slash: SlashState,
    /// Ranks the menu. Kept alive between keystrokes — it owns a scoring slab that is
    /// cheap to reuse and wasteful to rebuild per character.
    matcher: FuzzyMatcher,
    /// Set when the loop should exit after the current iteration.
    pub should_quit: bool,
    /// Redraw needed.
    pub dirty: bool,
    /// Run lifecycle.
    pub run: RunState,
    /// Finalized blocks awaiting folding into the transcript tail (drained by
    /// the loop's `flush_outbox`; ADR-0022).
    pub outbox: Vec<Block>,
    /// `--debug-show-hidden-context`: surface the parts of the request the UI hides —
    /// the preamble, the injected `<system-reminder>`s, and the tool schemas. Off by
    /// default; nothing about the request changes when it is on.
    pub show_hidden_context: bool,
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
    pub prompt_queue: VecDeque<QueuedPrompt>,
    /// The running session's mid-run input queue (ADR-0028). `Some` only while
    /// a run is active; pushing into it delivers the text at the next
    /// tool-result boundary instead of at turn end.
    pub input_queue: Option<locode_core::InputQueue>,
    /// How many of `prompt_queue`'s front entries were handed to the current
    /// run's engine queue. Needed because "the engine queue is empty" alone
    /// cannot distinguish *the engine took them* from *nothing was ever
    /// pushed* — conflating the two would silently drop a prompt queued for a
    /// later turn.
    pub mid_run_pushed: usize,
    /// Prompt history, most-recent-first (move-to-front dedup, cap 200).
    history: Vec<String>,
    /// History browse cursor (`None` = not browsing); index into `history`.
    history_nav: Option<usize>,
    /// The live draft saved when history browsing began (restored on exit).
    history_saved: Option<String>,
    /// Resolved model id (status display); `None` until the engine is ready.
    pub model: Option<String>,
    /// The effort rung in use (`None` = no override — the API's own default).
    pub effort: Option<locode_core::Effort>,
    /// The wire in use, so the effort menu can show each rung's mapping.
    pub api_schema: Option<String>,
    /// Working directory, home-shortened (status display); set at engine ready.
    pub cwd: Option<String>,
    /// Shell `run_terminal_cmd` uses (status display); set at engine ready.
    pub shell: Option<String>,
    /// The **current context occupancy**: what the last request actually carried
    /// (input + both cache counters + output — i.e. what the next turn starts from),
    /// not a cumulative generation total. Survives resume via an estimate until the
    /// first real usage report replaces it.
    pub context_tokens: u64,
    /// Whether `context_tokens` is a resume-time estimate (rendered `~N`).
    pub context_estimated: bool,
    /// Session assembly failed — submits are disabled.
    pub engine_failed: bool,
    /// Spinner frame counter (advanced by `Msg::Tick`).
    pub spinner_frame: usize,
    /// The **in-progress (uncommitted)** assistant text of a streaming turn
    /// (ADR-0021): deltas accumulate here and render in a live cell. The loop
    /// (`fold_streaming`) commits *completed* markdown blocks out of this into the
    /// transcript tail (→ native scrollback, so a long reply is scrollable
    /// mid-stream), leaving only the current block. `None` when not streaming.
    pub streaming: Option<String>,
    /// Whether any block of the current streaming message has already committed
    /// to the tail — so the `●` bullet (placed on the message's first line) isn't
    /// repeated on continuation chunks or the live cell.
    pub streaming_committed_any: bool,
    /// Set when the whole assistant `Message` has arrived: the loop commits the
    /// remaining in-progress block and clears the streaming state.
    pub streaming_finalize: bool,
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
        let mut registry = CommandRegistry::new();
        register_builtins(&mut registry);
        Self {
            composer: Composer::new(),
            effort: None,
            api_schema: None,
            registry,
            slash: SlashState::default(),
            matcher: FuzzyMatcher::new(),
            should_quit: false,
            dirty: true,
            run: RunState::Idle,
            outbox: Vec::new(),
            show_hidden_context: false,
            pending_tools: Vec::new(),
            approval_queue: VecDeque::new(),
            stashed_draft: None,
            prompt_queue: VecDeque::new(),
            input_queue: None,
            mid_run_pushed: 0,
            history: Vec::new(),
            history_nav: None,
            history_saved: None,
            model: None,
            cwd: None,
            shell: None,
            context_tokens: 0,
            context_estimated: false,
            engine_failed: false,
            spinner_frame: 0,
            streaming: None,
            streaming_committed_any: false,
            streaming_finalize: false,
            quit_armed_until: None,
            esc_armed_until: None,
            hint: None,
        }
    }

    /// Enable `--debug-show-hidden-context` (builder-style, so the constructors stay
    /// argument-free for the many tests that do not care).
    #[must_use]
    pub fn showing_hidden_context(mut self) -> Self {
        self.show_hidden_context = true;
        self
    }

    /// Fresh state with the composer pre-filled from a positional prompt
    /// (bare `locode "task"` — the user edits/sends it; it is not auto-sent).
    #[must_use]
    pub fn with_draft(draft: &str) -> Self {
        let mut app = Self::new();
        if !draft.trim().is_empty() {
            app.composer.set_text(draft);
            app.refresh_slash();
        }
        app
    }

    /// Re-derive the command menu from the composer. Called after **every** edit —
    /// the menu is a pure function of the draft and the cursor, never a thing that is
    /// separately opened and closed.
    fn refresh_slash(&mut self) {
        let Some(cursor) = self.composer.cursor_offset() else {
            self.slash.close();
            return;
        };
        let text = self.composer.text();
        // Built inline rather than via `command_ctx`: that borrows all of `self`, and
        // this call needs `slash` and `matcher` mutably alongside it.
        let ctx = CommandCtx {
            model: self.model.as_deref(),
            effort: self.effort,
            api_schema: self.api_schema.as_deref(),
            is_running: matches!(self.run, RunState::Running { .. }),
            registry: Some(&self.registry),
        };
        self.slash
            .refresh(&self.registry, &mut self.matcher, &ctx, &text, cursor);
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
                // Ignore key-release events (we don't enable REPORT_EVENT_TYPES,
                // so they shouldn't arrive — but a terminal that sends them must
                // not double every keypress). Press and Repeat both act.
                CrosstermEvent::Key(key) if key.kind == crossterm::event::KeyEventKind::Release => {
                    vec![]
                }
                CrosstermEvent::Key(key) => self.on_key(key, now),
                CrosstermEvent::Paste(text) => {
                    // Normalize CR pastes (Windows/legacy terminals) to LF.
                    self.composer
                        .insert_text(&text.replace("\r\n", "\n").replace('\r', "\n"));
                    self.refresh_slash();
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
            EngineMsg::Ready {
                model,
                cwd,
                shell,
                context,
                skills,
            } => {
                self.model = Some(model);
                self.cwd = Some(cwd);
                self.shell = Some(shell);
                self.register_skill_commands(&skills);
                // Fresh session: context resets to 0. Resumed: exact when the
                // rollout carried usage records, else a `~` estimate — either
                // way replaced by the first real usage report.
                if let Some(recovered) = context {
                    self.context_tokens = recovered.tokens;
                    self.context_estimated = recovered.estimated;
                } else {
                    self.context_tokens = 0;
                    self.context_estimated = false;
                }
                vec![]
            }
            // `/model` finished. The status bar takes the model the engine actually
            // resolved, so a refused or redirected switch cannot leave it claiming one
            // the session is not on.
            EngineMsg::Notice(message) => {
                self.outbox.push(Block::Notice(message));
                vec![]
            }
            EngineMsg::EffortChanged { effort, message } => {
                self.effort = effort;
                self.outbox.push(Block::Notice(message));
                vec![]
            }
            EngineMsg::ModelChanged { model, message } => {
                self.model = Some(model);
                self.outbox.push(Block::Notice(message));
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
            EngineMsg::RunStarted { input_queue, .. } => {
                self.input_queue = Some(input_queue);
                self.mid_run_pushed = 0;
                self.run = RunState::Running {
                    started: now,
                    cancelling: false,
                };
                self.hint = None;
                vec![]
            }
            EngineMsg::ReplayedPrompt(text) => {
                // A resumed transcript's user prompt — same cell as a live
                // submit echo.
                self.outbox.push(Block::UserPrompt(text));
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
            // A streaming assistant-text fragment: accumulate into the live cell
            // (ADR-0021). The whole `Message` still lands below and finalizes it.
            Event::MessageDelta { text } => {
                self.streaming.get_or_insert_default().push_str(&text);
            }
            // The stream was abandoned and the turn is being re-sampled: the same
            // reply is about to arrive again from the start, so drop the buffered
            // partial instead of letting the retry append to it. Rows already
            // committed to scrollback stay — they cannot be withdrawn — so a long
            // partial can still be visible above the re-streamed reply; the notice
            // the engine emits alongside is what explains it. `_committed_any`
            // resets too, so the re-stream renders with its speaker prefix again.
            Event::MessageDeltaReset { .. } => {
                self.streaming = None;
                self.streaming_committed_any = false;
                self.streaming_finalize = false;
            }
            // Injected framing (project instructions, the skills listing) is a `User`
            // message the UI drops, because live submits echo their own text. Under the
            // debug flag it is exactly what the user asked to see.
            Event::Message { ref message }
                if self.show_hidden_context
                    && message.role == Role::User
                    && message_text(message).starts_with("<system-reminder>") =>
            {
                self.outbox.push(Block::HiddenContext {
                    label: "injected reminder".to_string(),
                    body: message_text(message),
                });
            }
            Event::Message { message } => match message.role {
                Role::Assistant if self.streaming.is_some() => {
                    // Streaming turn: the assistant text already streamed and its
                    // completed blocks are committed; signal the loop to commit the
                    // last in-progress block (`fold_streaming`) rather than pushing a
                    // duplicate whole-text block. Tool calls still register here.
                    self.streaming_finalize = true;
                    for block in message.content {
                        if let ContentBlock::ToolUse { id, name, input } = block {
                            self.pending_tools.push(PendingTool {
                                id,
                                name,
                                args: args_summary(&input),
                            });
                        }
                    }
                }
                Role::Assistant => {
                    // Non-streaming turn: render the whole text as one block.
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
                    // Finalize the batch's tool cells first, then echo any
                    // mid-run input the engine appended to the SAME message.
                    // Reading it off the message rather than polling is what
                    // makes the order correct by construction: the text sits
                    // after the results on the wire, so it renders after their
                    // cells (ADR-0028 — transcript position = wire position).
                    let mut delivered_mid_run = false;
                    for block in message.content {
                        match block {
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => self.finalize_tool(&tool_use_id, &content, is_error),
                            ContentBlock::Text { text }
                                if text.starts_with(locode_core::MID_RUN_PREAMBLE) =>
                            {
                                delivered_mid_run = true;
                            }
                            _ => {}
                        }
                    }
                    if delivered_mid_run {
                        self.echo_delivered_mid_run();
                    }
                }
                _ => {}
            },
            Event::Error { message } => self.outbox.push(Block::Notice(message)),
            // The preamble and the tool schemas are otherwise invisible: they ride
            // `Init` and are never part of the transcript the UI draws.
            Event::Init {
                preamble, tools, ..
            } if self.show_hidden_context => {
                for msg in &preamble {
                    self.outbox.push(Block::HiddenContext {
                        label: format!("{:?} message", msg.role).to_lowercase(),
                        body: message_text(msg),
                    });
                }
                // In full, deliberately: a truncated schema cannot answer "what did the
                // model actually see?", which is the only reason this flag exists.
                for tool in &tools {
                    self.outbox.push(Block::HiddenContext {
                        label: format!("tool schema{}", tool_label(tool)),
                        body: json_fence(tool),
                    });
                }
            }
            // Init is chrome-irrelevant otherwise; Approval lands in slice 4;
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
        // Surface why a run failed (ModelError/Error) so a real-wire session
        // is legible — rate limit, bad key, fatal tool (codex adds an error
        // cell on a failed turn). Cancelled/Completed carry no error.
        if let Some(err) = &report.error {
            self.outbox.push(Block::Notice(format!("error: {err}")));
        }
        let elapsed = match self.run {
            RunState::Running { started, .. } => now.duration_since(started).as_secs(),
            RunState::Idle => 0,
        };
        // Context occupancy = the **final** turn's full prompt (input + both cache
        // counters) plus what it appended. `report.usage` is the run's *sum*, which
        // counts the same history once per turn and says nothing about how full the
        // window is. A real report replaces any estimate.
        self.context_tokens = report.context_usage.context_tokens();
        self.context_estimated = false;
        // A cancelled/errored streaming turn never emits the whole `Message`, so
        // no finalize is pending: drop the in-progress (uncommitted) block (Q2 —
        // the partial is discarded). Any *completed* blocks already committed to
        // scrollback stay (they can't be un-committed, and are real output). When
        // a finalize IS pending (normal completion), leave the state for
        // `fold_streaming` to commit the last block.
        if !self.streaming_finalize {
            self.streaming = None;
            self.streaming_committed_any = false;
        }
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

    /// Echo the mid-run prompts the engine just delivered, and retire them from
    /// the pending list (ADR-0028).
    ///
    /// Driven by the tool-result message that *carries* the text, not by
    /// polling: the engine appends it after the results, so echoing while
    /// handling that message puts it after the batch's tool cells — the same
    /// order the wire has. Polling on every update raced the message handler
    /// and rendered it one batch early.
    fn echo_delivered_mid_run(&mut self) {
        if self.mid_run_pushed == 0 {
            return;
        }
        {
            let n = self.mid_run_pushed.min(self.prompt_queue.len());
            for queued in self.prompt_queue.drain(..n) {
                self.outbox.push(Block::UserPrompt(queued.display));
            }
            self.mid_run_pushed = 0;
        }
    }

    /// Pop and submit the next queued prompt, if any (called at turn end).
    fn drain_queued_prompt(&mut self) -> Vec<Cmd> {
        // Delivered mid-run: the engine took the batch, so the transcript
        // already carries it and re-submitting would duplicate it. The engine
        // takes all-or-nothing, so an empty queue means every handed-over item
        // landed — but only the ones we actually handed over. Entries queued
        // for a later turn stay put.
        let delivered = self.mid_run_pushed > 0
            && self
                .input_queue
                .as_ref()
                .is_some_and(locode_core::InputQueue::is_empty);
        self.input_queue = None;
        if delivered {
            self.prompt_queue
                .drain(..self.mid_run_pushed.min(self.prompt_queue.len()));
            self.mid_run_pushed = 0;
            return vec![];
        }
        self.mid_run_pushed = 0;
        match self.prompt_queue.pop_front() {
            // Never taken by the engine, so this is where it enters the
            // conversation — echo it here.
            Some(queued) => {
                self.outbox.push(Block::UserPrompt(queued.display));
                vec![Cmd::Submit(queued.wire)]
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

    /// One keystroke, then re-derive the command menu — every path that can touch the
    /// composer funnels through here, so the menu can never go stale.
    fn on_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Cmd> {
        let cmds = self.on_key_inner(key, now);
        self.refresh_slash();
        cmds
    }

    /// Navigation and acceptance while the command menu is open (grok's intercept,
    /// `agent_view/prompt.rs:144-231`), which runs **before** the ordinary bindings so
    /// Esc dismisses the menu instead of cancelling the run and ↑/↓ move the highlight
    /// instead of browsing history.
    ///
    /// `None` means "not mine" — the key falls through to the ordinary handling. Enter
    /// deliberately returns `None` after completing a terminal row: the completed text
    /// then rides the normal submit path.
    fn on_slash_key(&mut self, key: KeyEvent, ctrl: bool) -> Option<Vec<Cmd>> {
        match (key.code, ctrl) {
            (KeyCode::Up, false) | (KeyCode::Char('p'), true) => {
                self.slash.move_selection(-1);
                Some(vec![])
            }
            (KeyCode::Down, false) | (KeyCode::Char('n'), true) => {
                self.slash.move_selection(1);
                Some(vec![])
            }
            // Tab completes the text and nothing else — never executes.
            (KeyCode::Tab, false) => {
                self.accept_slash();
                Some(vec![])
            }
            (KeyCode::Esc, _) => {
                let text = self.composer.text();
                self.slash.dismiss(&text);
                Some(vec![])
            }
            (KeyCode::Enter, false)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                // A row that completes to a trailing space expects more typing, so
                // Enter fills it in and waits; anything else falls through and submits.
                let chains = self.slash.selection_chains();
                self.accept_slash();
                if chains {
                    Some(vec![])
                } else {
                    self.slash.close();
                    None
                }
            }
            _ => None,
        }
    }

    /// Replace the command token with the selected row.
    fn accept_slash(&mut self) {
        if let Some((range, text)) = self.slash.accept() {
            self.composer.replace_range(range, &text);
        }
    }

    fn on_key_inner(&mut self, key: KeyEvent, now: Instant) -> Vec<Cmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // The approval overlay owns non-Ctrl input; Ctrl+C/Ctrl+D still fall
        // through to cancel/quit (cancel drains the queue).
        if self.is_awaiting_approval() && !ctrl {
            return self.on_approval_key(key);
        }
        if self.slash.open
            && let Some(cmds) = self.on_slash_key(key, ctrl)
        {
            return cmds;
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
                if let Some(queued) = self.prompt_queue.pop_back() {
                    // Un-queue the most recently queued prompt (codex's
                    // edit-queued gesture, mapped to Esc per our spec). The *display*
                    // text goes back, so an un-queued `/commit foo` is the command
                    // again rather than the skill body it expanded to.
                    self.composer.set_text(&queued.display);
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
            // Enter submits; Alt+Enter (always) or Shift+Enter insert a newline.
            // Alt+Enter works on any terminal; Shift+Enter only when the terminal
            // reports the modifier on Enter (needs the kitty keyboard protocol —
            // enabling it repo-wide is deferred, see the TUI polish backlog).
            (KeyCode::Enter, _) => self.on_enter(key),
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

    /// Enter: submit the draft, or insert a newline under Alt/Shift.
    fn on_enter(&mut self, key: KeyEvent) -> Vec<Cmd> {
        if key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
        {
            self.composer.insert_newline();
            return vec![];
        }
        let text = self.composer.take_text();
        self.disarm();
        self.history_nav = None;
        if text.trim().is_empty() {
            return vec![];
        }
        self.record_history(&text);
        // Slash commands intercept before submit/queue — including when the engine is
        // not ready, so `/quit` still works on a session that failed to build.
        if let Some(cmds) = Self::try_slash(&text) {
            return cmds;
        }
        if self.engine_failed || self.model.is_none() {
            self.composer.set_text(&text); // engine not ready; keep it
            return vec![];
        }
        self.send(QueuedPrompt::plain(text))
    }

    /// Send a prompt: straight to the engine when idle, onto the queue while a run is
    /// active (codex's queue-and-drain). Either way the transcript echoes it once.
    fn send(&mut self, prompt: QueuedPrompt) -> Vec<Cmd> {
        if self.is_running() {
            // Hand it to the engine so it lands at the next tool-result
            // boundary rather than waiting out the whole run (ADR-0028). The
            // local copy stays for rendering and for the no-carrier fallback:
            // a turn that emits no tool calls leaves the item undrained, and
            // `drain_queued_prompt` then submits it as an ordinary prompt.
            if let Some(queue) = &self.input_queue {
                queue.push(prompt.wire.clone());
                self.mid_run_pushed += 1;
            }
            // No transcript echo yet — queued is not delivered. The pending
            // list renders it meanwhile; the transcript gets it at the point
            // the engine actually takes it, which is a later tool round.
            self.prompt_queue.push_back(prompt);
            return vec![];
        }
        self.outbox.push(Block::UserPrompt(prompt.display));
        vec![Cmd::Submit(prompt.wire)]
    }

    /// Hand a `/name args` line to the registry; `None` when `text` is not an
    /// invocation at all (a path, a bare slash) and should ride the ordinary prompt
    /// path — ADR-0026 §5.
    fn try_slash(text: &str) -> Option<Vec<Cmd>> {
        let trimmed = text.trim();
        parse_invocation(trimmed)?;
        Some(vec![Cmd::RunCommand {
            line: trimmed.to_string(),
        }])
    }

    /// Rebuild the registry with the builtins plus this session's skills.
    ///
    /// Rebuilt rather than appended to, because `/new` reports a fresh set and a
    /// deleted skill must stop being offered. Builtins go first: registration is
    /// first-wins, which is what makes builtins beat skills (ADR-0026 §4).
    ///
    /// The list is the one the engine discovered when it assembled the session, so the
    /// menu and the model's listing never disagree. A skill added mid-session reaches
    /// the model on the next turn (ADR-0025 §3.2's rescan) but the menu only on the
    /// next `/new` — recorded as the known gap rather than a second rescan path.
    fn register_skill_commands(&mut self, skills: &[locode_skills::Skill]) {
        let mut registry = CommandRegistry::new();
        register_builtins(&mut registry);
        register_skills(&mut registry, skills);
        self.registry = registry;
    }

    /// The read-only view commands get of the session.
    #[must_use]
    pub fn command_ctx(&self) -> CommandCtx<'_> {
        CommandCtx {
            effort: self.effort,
            api_schema: self.api_schema.as_deref(),
            model: self.model.as_deref(),
            is_running: matches!(self.run, RunState::Running { .. }),
            registry: Some(&self.registry),
        }
    }

    /// Apply what a command returned. The command itself touched nothing
    /// (ADR-0026 §2); this is where its effect actually happens.
    pub fn apply_command_result(&mut self, result: CommandResult) -> Vec<Cmd> {
        self.dirty = true;
        match result {
            CommandResult::Handled => vec![],
            // Both land as a notice; the difference is the wording the command chose,
            // not a second rendering path (grok pushes a system block for either).
            CommandResult::Message(text) | CommandResult::Error(text) => {
                self.outbox.push(Block::Notice(text));
                vec![]
            }
            CommandResult::Prompt(text) => {
                if self.engine_failed || self.model.is_none() {
                    self.outbox
                        .push(Block::Notice("the session is not ready yet".into()));
                    return vec![];
                }
                self.send(QueuedPrompt::plain(text))
            }
            // The transcript shows the invocation; the model receives the body.
            CommandResult::InjectSkill {
                display_text,
                prompt_text,
            } => {
                if self.engine_failed || self.model.is_none() {
                    self.outbox
                        .push(Block::Notice("the session is not ready yet".into()));
                    return vec![];
                }
                self.send(QueuedPrompt {
                    display: display_text,
                    wire: prompt_text,
                })
            }
            CommandResult::Action(UiAction::NewSession) => vec![Cmd::NewSession],
            CommandResult::Action(UiAction::Quit) => {
                self.should_quit = true;
                vec![Cmd::Quit]
            }
            // Not applied here: the swap rebuilds a provider, which is IO. The footer
            // follows when the engine task reports the model it actually resolved,
            // so a failed build never leaves the status bar claiming a model that is
            // not in use.
            CommandResult::Action(UiAction::SetModel(model)) => vec![Cmd::SetModel(model)],
            CommandResult::Action(UiAction::SetEffort(effort)) => vec![Cmd::SetEffort(effort)],
            CommandResult::Action(UiAction::AddDir(dir)) => vec![Cmd::AddDir(dir)],
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

/// `: <name>` when the tool spec carries one, else empty — the schemas are otherwise
/// indistinguishable at a glance.
fn tool_label(tool: &serde_json::Value) -> String {
    tool.get("name")
        .and_then(serde_json::Value::as_str)
        .map(|n| format!(": {n}"))
        .unwrap_or_default()
}

/// Pretty-print a tool schema into a fenced `json` block (two-space indent).
///
/// The fence routes it through the markdown renderer's syntect highlighting and
/// wrapping instead of spilling one long line off the right edge. Key order is left
/// exactly as the schema declares it — that ordering is part of what was sent, and
/// showing what was sent is the only reason this flag exists.
fn json_fence(value: &serde_json::Value) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    format!("```json\n{pretty}\n```")
}

/// Flatten a message's text blocks — used only by `--debug-show-hidden-context`, which
/// prints content the UI otherwise never draws.
fn message_text(msg: &locode_core::Message) -> String {
    msg.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    /// The message the engine emits when it drains queued input: the batch's
    /// tool results, then the marked text appended after them.
    fn mid_run_carrier(tool_id: &str, text: &str) -> Msg {
        Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::Message {
            message: Message {
                role: Role::User,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: tool_id.into(),
                        content: vec![ResultChunk::Text { text: "ok".into() }],
                        is_error: false,
                    },
                    ContentBlock::Text {
                        text: format!("{}:\n\n{text}", locode_core::MID_RUN_PREAMBLE),
                    },
                ],
            },
        }))))
    }

    fn run_started() -> Msg {
        Msg::Engine(Box::new(EngineMsg::RunStarted {
            cancel: locode_core::CancellationToken::new(),
            input_queue: locode_core::InputQueue::new(),
        }))
    }
    fn type_str(app: &mut App, s: &str, now: Instant) {
        for ch in s.chars() {
            let _ = app.update(key(KeyCode::Char(ch)), now);
        }
    }
    /// Drive one message the way the event loop does (`event_loop::run_reducer`): run
    /// the reducer, execute whatever command it asked for, apply the result, and return
    /// only the commands that actually reach the loop's IO.
    async fn drive(app: &mut App, msg: Msg, now: Instant) -> Vec<Cmd> {
        let mut out = Vec::new();
        let mut work: VecDeque<Cmd> = app.update(msg, now).into();
        while let Some(cmd) = work.pop_front() {
            if let Cmd::RunCommand { line } = cmd {
                let ctx = app.command_ctx();
                let result = crate::commands::execute(&app.registry, &ctx, &line).await;
                work.extend(app.apply_command_result(result));
            } else {
                out.push(cmd);
            }
        }
        out
    }
    fn ready_app() -> App {
        let mut app = App::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Ready {
                context: None,
                model: "mock-1".into(),
                cwd: "~/proj".into(),
                shell: "zsh".into(),
                skills: Vec::new(),
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
            // The run's sum; `context_usage` is the last turn's, deliberately different
            // so a test can tell which one the footer reads.
            usage: Usage {
                input_tokens: 100,
                output_tokens: 20,
                ..Usage::default()
            },
            context_usage: Usage {
                input_tokens: 60,
                output_tokens: 12,
                cache_read_tokens: Some(30),
                cache_creation_tokens: Some(8),
                ..Usage::default()
            },
            session_id: "s".into(),
            stop_reason: None,
            error: None,
        }
    }

    /// The footer shows **context occupancy**, not accumulated usage: the final turn's
    /// whole prompt (input plus both cache counters) plus its completion.
    ///
    /// Reading `report.usage` would show the run's sum, which counts the same
    /// conversation once per turn and only ever grows — a number with no relationship
    /// A resample after a mid-stream failure re-runs the same request, so the
    /// same reply streams again from the start. Without dropping the partial the
    /// second stream appends to the first and the user reads the reply twice.
    #[test]
    fn a_delta_reset_drops_the_partial_so_the_resample_does_not_double_it() {
        let mut app = App::new();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        for frag in ["Hel", "lo wor"] {
            let _ = app.update(
                Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::MessageDelta {
                    text: frag.into(),
                })))),
                t0,
            );
        }
        assert_eq!(app.streaming.as_deref(), Some("Hello wor"));

        // The stream failed; the engine is about to resample the same request.
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(
                Event::MessageDeltaReset {
                    reason: "lossy stream".into(),
                },
            )))),
            t0,
        );
        assert!(app.streaming.is_none(), "the void partial is dropped");
        assert!(!app.streaming_committed_any);
        assert!(!app.streaming_finalize);

        // The retry streams the whole reply — and it stands alone.
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::MessageDelta {
                text: "Hello world".into(),
            })))),
            t0,
        );
        assert_eq!(
            app.streaming.as_deref(),
            Some("Hello world"),
            "the retry must not append to the abandoned partial"
        );
    }

    /// to the context window.
    #[test]
    fn the_token_counter_is_the_last_turn_not_the_run_total() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        assert_eq!(app.context_tokens, 60 + 30 + 8 + 12);
        assert!(!app.context_estimated);

        // A second run does not add to it — occupancy is replaced, never accumulated.
        let _ = app.update(run_started(), t0);
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        assert_eq!(app.context_tokens, 60 + 30 + 8 + 12, "replaced, not summed");
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
    fn shift_enter_inserts_newline_like_alt_enter() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "a", t0);
        let shift_enter = Msg::Input(Box::new(CrosstermEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::SHIFT,
        ))));
        let cmds = app.update(shift_enter, t0);
        assert_eq!(cmds, vec![], "shift+enter does not submit");
        type_str(&mut app, "b", t0);
        assert_eq!(
            app.update(key(KeyCode::Enter), t0),
            vec![Cmd::Submit("a\nb".into())]
        );
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let release = Msg::Input(Box::new(CrosstermEvent::Key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Release,
            state: crossterm::event::KeyEventState::NONE,
        })));
        let cmds = app.update(release, t0);
        assert_eq!(cmds, vec![]);
        assert!(app.composer.is_empty(), "release must not type a char");
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

    // ---- streaming live cell (ADR-0021 slice 1c) ----

    #[test]
    fn streaming_deltas_accumulate_then_finalize_to_a_block() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        for frag in ["Hello ", "streamed ", "world"] {
            let _ = app.update(
                Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::MessageDelta {
                    text: frag.into(),
                })))),
                t0,
            );
        }
        // Deltas live in the (uncommitted) streaming buffer; nothing in outbox.
        assert_eq!(app.streaming.as_deref(), Some("Hello streamed world"));
        assert!(
            app.outbox.is_empty(),
            "nothing pushed to outbox while streaming"
        );

        // The whole Message lands → the reducer signals `fold_streaming` (the loop)
        // to commit the remaining block; it does NOT push a duplicate whole block.
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::Message {
                message: Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Hello streamed world".into(),
                    }],
                },
            })))),
            t0,
        );
        assert!(app.streaming_finalize, "finalize signalled to the loop");
        assert_eq!(
            app.streaming.as_deref(),
            Some("Hello streamed world"),
            "text kept for the loop to commit"
        );
        assert!(
            app.outbox.is_empty(),
            "no duplicate whole-text block pushed: {:?}",
            app.outbox
        );
    }

    #[test]
    fn run_finished_discards_a_partial_streaming_cell() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::MessageDelta {
                text: "partial".into(),
            })))),
            t0,
        );
        assert_eq!(app.streaming.as_deref(), Some("partial"));
        // A cancelled turn ends without the whole Message → drop the partial (Q2).
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Cancelled,
            ))))),
            t0,
        );
        assert_eq!(app.streaming, None, "partial discarded on run end");
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
        // Echoed when QUEUED, not at turn end — once the engine drains a prompt
        // mid-run there is no turn-end submit left to echo from (ADR-0028).
        // Never taken by the engine, so the fallback submit is where it enters
        // the conversation — and that is where it is echoed.
        assert!(matches!(app.outbox.last(), Some(Block::UserPrompt(p)) if p == "next prompt"));
    }

    /// Regression: a mid-run prompt the engine delivers must still be visible.
    /// It was echoed nowhere — `send` skipped the echo while running, and the
    /// delivered path submits nothing, so the only echo site never ran.
    #[test]
    fn a_delivered_mid_run_prompt_is_echoed_into_the_transcript() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let queue = locode_core::InputQueue::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunStarted {
                cancel: locode_core::CancellationToken::new(),
                input_queue: queue.clone(),
            })),
            t0,
        );
        type_str(&mut app, "what is the date?", t0);
        let _ = app.update(key(KeyCode::Enter), t0);

        // NOT in the transcript yet — queued is not delivered. It renders in
        // the pending list until the engine actually takes it.
        assert!(
            !app.outbox
                .iter()
                .any(|b| matches!(b, Block::UserPrompt(p) if p == "what is the date?")),
            "queued must not appear in the transcript"
        );
        assert_eq!(app.prompt_queue.len(), 1);

        // The engine takes it and appends it to a tool-result batch — the
        // message that carries it is what drives the echo.
        let _ = queue.take_all();
        let _ = app.update(mid_run_carrier("c1", "what is the date?"), t0);
        assert!(
            matches!(app.outbox.last(), Some(Block::UserPrompt(p)) if p == "what is the date?"),
            "echoed at the point it actually entered the conversation"
        );
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        let echoes = app
            .outbox
            .iter()
            .filter(|b| matches!(b, Block::UserPrompt(p) if p == "what is the date?"))
            .count();
        assert_eq!(echoes, 1, "exactly once — not zero, not duplicated");
    }

    /// Regression: the echo must land **after** the batch's tool cells, matching
    /// the wire, where the text sits after the tool results. Polling for
    /// delivery raced the message handler and rendered it a batch early.
    #[test]
    fn the_echo_lands_after_the_tool_cells_of_its_own_batch() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let queue = locode_core::InputQueue::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunStarted {
                cancel: locode_core::CancellationToken::new(),
                input_queue: queue.clone(),
            })),
            t0,
        );
        // A tool is in flight when the user types.
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Event(Box::new(Event::Message {
                message: Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "run_terminal_cmd".into(),
                        input: serde_json::json!({}),
                    }],
                },
            })))),
            t0,
        );
        type_str(&mut app, "and the date?", t0);
        let _ = app.update(key(KeyCode::Enter), t0);

        let _ = queue.take_all();
        let _ = app.update(mid_run_carrier("c1", "and the date?"), t0);

        let order: Vec<&str> = app
            .outbox
            .iter()
            .filter_map(|b| match b {
                Block::ToolCall { .. } => Some("tool"),
                Block::UserPrompt(p) if p == "and the date?" => Some("prompt"),
                _ => None,
            })
            .collect();
        assert_eq!(
            order.last(),
            Some(&"prompt"),
            "the prompt follows its batch's tool cell, as it does on the wire: {order:?}"
        );
        assert!(order.contains(&"tool"), "the tool cell rendered: {order:?}");
    }

    /// Regression: the pending list must stop showing a prompt the engine has
    /// already taken, or the UI reports it as queued forever.
    #[test]
    fn a_delivered_prompt_stops_rendering_as_queued() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let queue = locode_core::InputQueue::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunStarted {
                cancel: locode_core::CancellationToken::new(),
                input_queue: queue.clone(),
            })),
            t0,
        );
        type_str(&mut app, "mid-run", t0);
        let _ = app.update(key(KeyCode::Enter), t0);
        assert_eq!(
            app.prompt_queue.len(),
            1,
            "pending while the engine holds it"
        );

        // The engine drains mid-run — still the same run, no RunFinished yet.
        let _ = queue.take_all();
        let _ = app.update(mid_run_carrier("c1", "mid-run"), t0);
        assert!(
            app.prompt_queue.is_empty(),
            "on the wire now, so no longer pending"
        );
    }

    /// A prompt typed mid-run is handed to the engine's queue so it lands at
    /// the next tool-result boundary (ADR-0028), not held until turn end.
    #[test]
    fn a_mid_run_prompt_goes_to_the_engine_queue() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let queue = locode_core::InputQueue::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunStarted {
                cancel: locode_core::CancellationToken::new(),
                input_queue: queue.clone(),
            })),
            t0,
        );
        type_str(&mut app, "by the way, use tabs", t0);
        assert_eq!(app.update(key(KeyCode::Enter), t0), vec![]);

        assert_eq!(
            queue.pending(),
            vec!["by the way, use tabs".to_string()],
            "handed to the running session, not merely parked locally"
        );
        assert_eq!(app.mid_run_pushed, 1);
    }

    /// When the engine drained it, turn end must NOT resubmit — the transcript
    /// already carries it.
    #[test]
    fn a_delivered_mid_run_prompt_is_not_resubmitted() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let queue = locode_core::InputQueue::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunStarted {
                cancel: locode_core::CancellationToken::new(),
                input_queue: queue.clone(),
            })),
            t0,
        );
        type_str(&mut app, "mid-run note", t0);
        let _ = app.update(key(KeyCode::Enter), t0);

        // The engine takes the batch mid-run.
        let _ = queue.take_all();

        let cmds = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        assert_eq!(cmds, vec![], "already delivered — no second submit");
        assert!(app.prompt_queue.is_empty());
    }

    /// A prompt the engine never took (a turn with no tool calls, so no batch
    /// to ride) still reaches the model as an ordinary next-turn prompt.
    #[test]
    fn an_undelivered_mid_run_prompt_falls_back_to_a_normal_submit() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let queue = locode_core::InputQueue::new();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunStarted {
                cancel: locode_core::CancellationToken::new(),
                input_queue: queue,
            })),
            t0,
        );
        type_str(&mut app, "no carrier", t0);
        let _ = app.update(key(KeyCode::Enter), t0);

        // Queue left untouched — the run emitted no tool calls.
        let cmds = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        assert_eq!(cmds, vec![Cmd::Submit("no carrier".into())]);
    }

    #[test]
    fn esc_at_idle_pops_the_last_queued_prompt() {
        let mut app = ready_app();
        let t0 = Instant::now();
        app.prompt_queue
            .push_back(QueuedPrompt::plain("first".into()));
        app.prompt_queue
            .push_back(QueuedPrompt::plain("second".into()));
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

    #[tokio::test]
    async fn slash_quit_and_new_and_unknown() {
        let mut app = ready_app();
        let t0 = Instant::now();

        type_str(&mut app, "/quit", t0);
        assert_eq!(
            drive(&mut app, key(KeyCode::Enter), t0).await,
            vec![Cmd::Quit]
        );
        assert!(app.should_quit);

        let mut app = ready_app();
        type_str(&mut app, "/new", t0);
        assert_eq!(
            drive(&mut app, key(KeyCode::Enter), t0).await,
            vec![Cmd::NewSession]
        );

        // Unknown slash → notice naming the near miss, no command, not submitted.
        type_str(&mut app, "/nwe", t0);
        assert_eq!(drive(&mut app, key(KeyCode::Enter), t0).await, vec![]);
        let Some(Block::Notice(notice)) = app.outbox.last() else {
            panic!("expected a notice, got {:?}", app.outbox.last());
        };
        assert!(notice.contains("unknown command: /nwe"), "{notice}");

        // /new while running → the command refuses itself, no reset.
        let _ = app.update(run_started(), t0);
        type_str(&mut app, "/new", t0);
        assert_eq!(drive(&mut app, key(KeyCode::Enter), t0).await, vec![]);
        assert!(
            matches!(app.outbox.last(), Some(Block::Notice(n)) if n.contains("cancel the run"))
        );
    }

    /// A path is ordinary text, not a mistyped command (ADR-0026 §5).
    #[tokio::test]
    async fn a_leading_path_is_submitted_as_a_prompt() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/usr/bin/env is the one I mean", t0);
        assert_eq!(
            drive(&mut app, key(KeyCode::Enter), t0).await,
            vec![Cmd::Submit("/usr/bin/env is the one I mean".into())]
        );
    }

    /// End to end: a skill the engine discovered is offered in the menu, and invoking
    /// it sends the body while the transcript shows the invocation.
    #[tokio::test]
    async fn a_discovered_skill_is_offered_and_injects_its_body() {
        let dir = tempfile::TempDir::new().unwrap();
        let skill_dir = dir.path().join("commit");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        std::fs::write(
            &path,
            "---\nname: commit\ndescription: d\n---\nStage, then commit.\n",
        )
        .unwrap();

        let mut app = App::new();
        let t0 = Instant::now();
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::Ready {
                context: None,
                model: "mock-1".into(),
                cwd: "~/proj".into(),
                shell: "zsh".into(),
                skills: vec![locode_skills::Skill {
                    name: "commit".into(),
                    scope: locode_skills::SkillScope::Project,
                    description: "make a commit".into(),
                    when_to_use: None,
                    path,
                    disable_model_invocation: false,
                    user_invocable: true,
                }],
            })),
            t0,
        );

        type_str(&mut app, "/com", t0);
        assert_eq!(menu(&app), vec!["/commit"], "offered in the menu");

        type_str(&mut app, "mit fix the typo", t0);
        let cmds = drive(&mut app, key(KeyCode::Enter), t0).await;
        assert_eq!(
            cmds,
            vec![Cmd::Submit(
                "Stage, then commit.\n\n**ARGUMENTS:** fix the typo".into()
            )],
            "the model receives the body plus the arguments block"
        );
        assert_eq!(
            app.outbox.last(),
            Some(&Block::UserPrompt("/commit fix the typo".into())),
            "the transcript shows the invocation"
        );
    }

    /// A skill invocation splits what the transcript shows from what the model gets.
    #[tokio::test]
    async fn injecting_a_skill_echoes_the_invocation_and_sends_the_body() {
        let mut app = ready_app();
        let cmds = app.apply_command_result(CommandResult::InjectSkill {
            display_text: "/commit fix the typo".into(),
            prompt_text: "the skill body\n\n**ARGUMENTS:** fix the typo".into(),
        });
        assert_eq!(
            cmds,
            vec![Cmd::Submit(
                "the skill body\n\n**ARGUMENTS:** fix the typo".into()
            )],
            "the model receives the body"
        );
        assert_eq!(
            app.outbox.last(),
            Some(&Block::UserPrompt("/commit fix the typo".into())),
            "the transcript shows the invocation"
        );
    }

    /// Queued mid-run, the split survives: the preview shows the invocation and the
    /// engine still gets the body when the queue drains.
    #[tokio::test]
    async fn a_queued_skill_keeps_its_display_and_wire_texts_apart() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let cmds = app.apply_command_result(CommandResult::InjectSkill {
            display_text: "/commit x".into(),
            prompt_text: "BODY".into(),
        });
        assert_eq!(cmds, vec![], "queued, not submitted");
        assert_eq!(app.prompt_queue.len(), 1);
        assert_eq!(app.prompt_queue[0].display, "/commit x");

        // When the run ends and the queue drains, the transcript still shows the
        // invocation while the engine receives the body.
        let cmds = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        assert!(cmds.contains(&Cmd::Submit("BODY".into())), "{cmds:?}");
        assert!(
            app.outbox
                .iter()
                .any(|b| matches!(b, Block::UserPrompt(p) if p == "/commit x")),
            "the transcript shows the invocation: {:?}",
            app.outbox
        );
    }

    #[test]
    fn session_reset_clears_transcript_state() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        app.prompt_queue.push_back(QueuedPrompt::plain("q".into()));
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

    // ---- slice 6: error surfacing ----

    #[test]
    fn failed_run_surfaces_the_error_before_the_separator() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let mut r = report(Status::ModelError);
        r.error = Some("rate limited after 3 retries".into());
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(r)))),
            t0,
        );
        // error notice immediately precedes the TurnEnd separator.
        let n = app.outbox.len();
        assert!(
            matches!(&app.outbox[n - 2], Block::Notice(m) if m.contains("rate limited")),
            "{:?}",
            app.outbox
        );
        assert!(matches!(
            &app.outbox[n - 1],
            Block::TurnEnd {
                status: Status::ModelError,
                ..
            }
        ));
    }

    #[test]
    fn with_draft_prefills_the_composer_without_sending() {
        let app = App::with_draft("draft task");
        assert_eq!(app.composer.text(), "draft task");
        assert!(app.outbox.is_empty(), "pre-fill does not auto-send");
        // Empty/whitespace draft leaves the composer empty.
        assert!(App::with_draft("  ").composer.is_empty());
    }

    #[test]
    fn completed_run_has_no_error_notice() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::RunFinished(Box::new(report(
                Status::Completed,
            ))))),
            t0,
        );
        assert!(
            !app.outbox
                .iter()
                .any(|b| matches!(b, Block::Notice(m) if m.starts_with("error:"))),
            "no error notice on success"
        );
    }

    /// Off by default: none of the hidden parts reach the transcript.
    #[test]
    fn hidden_context_is_off_unless_asked_for() {
        let mut app = App::new();
        app.on_event(Event::Init {
            session_id: "s".into(),
            harness: "grok".into(),
            api_schema: "mock".into(),
            model: "m".into(),
            cwd: "/tmp".into(),
            max_turns: None,
            preamble: vec![locode_core::Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "SYSTEM PROMPT".into(),
                }],
            }],
            tools: vec![serde_json::json!({"name": "read_file"})],
        });
        assert!(app.outbox.is_empty(), "{:?}", app.outbox);
    }

    /// On: the preamble and the **full** tool schema both appear — a truncated schema
    /// cannot answer "what did the model actually see?".
    #[test]
    fn hidden_context_shows_preamble_and_full_tool_schemas() {
        let mut app = App::new().showing_hidden_context();
        app.on_event(Event::Init {
            session_id: "s".into(),
            harness: "grok".into(),
            api_schema: "mock".into(),
            model: "m".into(),
            cwd: "/tmp".into(),
            max_turns: None,
            preamble: vec![locode_core::Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "SYSTEM PROMPT".into(),
                }],
            }],
            tools: vec![serde_json::json!({
                "name": "read_file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            })],
        });
        let all = app
            .outbox
            .iter()
            .map(|b| format!("{b:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("SYSTEM PROMPT"), "{all}");
        assert!(all.contains("read_file"), "{all}");
        assert!(
            all.contains("input_schema"),
            "full schema, not just the name: {all}"
        );

        // Schemas arrive as a fenced `json` block so the markdown renderer wraps and
        // highlights them instead of spilling one long line off the right edge.
        let schema = app
            .outbox
            .iter()
            .find_map(|b| match b {
                Block::HiddenContext { label, body } if label.starts_with("tool schema") => {
                    Some((label.clone(), body.clone()))
                }
                _ => None,
            })
            .expect("a tool-schema block");
        assert_eq!(schema.0, "tool schema: read_file", "labeled by tool name");
        assert!(schema.1.starts_with("```json\n"), "{}", schema.1);
        // Two-space indent; key order is whatever the schema declared.
        assert!(schema.1.contains("\n  \"input_schema\""), "{}", schema.1);
    }

    /// Long hidden content wraps to the width instead of running off the edge — the
    /// bug this rendering pass exists to fix.
    #[test]
    fn hidden_context_wraps_to_the_available_width() {
        let block = Block::HiddenContext {
            label: "system message".into(),
            body: "word ".repeat(120),
        };
        let lines = block.render(60);
        assert!(lines.len() > 3, "wrapped: {}", lines.len());
        for line in &lines {
            let w: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(w <= 60, "line exceeds width: {w}");
        }
    }

    /// The marker distinguishes hidden context from conversation blocks at a glance.
    #[test]
    fn hidden_context_uses_its_own_marker() {
        let lines = Block::HiddenContext {
            label: "injected reminder".into(),
            body: "x".into(),
        }
        .render(80);
        // Every block gets the shared 2-col left margin, so compare after it.
        let head: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        let head = head.trim_start();
        assert!(
            head.starts_with("\u{2715} [hidden context] injected reminder"),
            "{head}"
        );
        assert!(!head.starts_with('\u{25cf}'), "not the bullet: {head}");
    }

    /// The other half of "hidden": injected `<system-reminder>` user messages, which the
    /// normal path drops because live submits echo their own text.
    #[test]
    fn hidden_context_shows_injected_reminders_but_not_ordinary_prompts() {
        let mut app = App::new().showing_hidden_context();
        let reminder = locode_core::Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "<system-reminder>\nThe following skills are available for use:\n</system-reminder>".into(),
            }],
        };
        let prompt = locode_core::Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "just my question".into(),
            }],
        };
        app.on_event(Event::Message { message: reminder });
        app.on_event(Event::Message { message: prompt });

        let all = app
            .outbox
            .iter()
            .map(|b| format!("{b:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("skills are available"), "{all}");
        assert!(
            !all.contains("just my question"),
            "own prompt not duplicated: {all}"
        );
    }

    // ── Slash-command menu (Task 34 S3) ─────────────────────────────────────

    /// Row labels currently offered by the menu.
    fn menu(app: &App) -> Vec<&str> {
        app.slash
            .matches
            .iter()
            .map(|r| r.display.as_str())
            .collect()
    }

    #[test]
    fn typing_a_slash_opens_the_menu_and_typing_on_narrows_it() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/", t0);
        assert!(app.slash.open, "a bare slash offers everything");
        assert_eq!(
            menu(&app),
            vec!["/add-dir", "/effort", "/help", "/model", "/new", "/quit"]
        );

        type_str(&mut app, "q", t0);
        assert_eq!(menu(&app), vec!["/quit"], "narrowed to the match");

        type_str(&mut app, "zz", t0);
        assert!(!app.slash.open, "no match closes the menu");
    }

    /// Ordinary text — including a path — must never open the menu.
    #[test]
    fn ordinary_text_leaves_the_menu_closed() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "look in /usr/bin", t0);
        assert!(!app.slash.open);
    }

    /// While the menu is open the arrows drive it, not the prompt history.
    #[test]
    fn arrows_move_the_selection_instead_of_browsing_history() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "earlier prompt", t0);
        let _ = app.update(key(KeyCode::Enter), t0);

        type_str(&mut app, "/", t0);
        let _ = app.update(key(KeyCode::Down), t0);
        assert_eq!(app.slash.selected, 1);
        assert_eq!(
            app.composer.text(),
            "/",
            "history did not overwrite the draft"
        );
        let _ = app.update(key(KeyCode::Up), t0);
        assert_eq!(app.slash.selected, 0);
    }

    /// grok's rule: Esc belongs to the menu first — dismissing it must not cancel the
    /// run underneath (`mid_turn_slash_dropdown_esc_dismisses_not_cancel`).
    #[test]
    fn esc_dismisses_the_menu_without_cancelling_the_run() {
        let mut app = ready_app();
        let t0 = Instant::now();
        let _ = app.update(run_started(), t0);
        type_str(&mut app, "/", t0);
        assert!(app.slash.open);

        assert_eq!(
            app.update(key(KeyCode::Esc), t0),
            vec![],
            "the first Esc only closes the menu"
        );
        assert!(!app.slash.open);
        assert!(app.is_running(), "run untouched");

        assert_eq!(
            app.update(key(KeyCode::Esc), t0),
            vec![Cmd::CancelRun],
            "the next Esc reaches the run"
        );
    }

    /// Tab completes the text and stops there — completion is never execution.
    #[test]
    fn tab_completes_without_running_the_command() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/qu", t0);
        assert_eq!(app.update(key(KeyCode::Tab), t0), vec![]);
        assert_eq!(app.composer.text(), "/quit");
        assert!(!app.should_quit, "completing is not running");
    }

    /// Enter on a partially typed command completes it *and* runs it.
    #[tokio::test]
    async fn enter_completes_the_selection_then_submits_it() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/qu", t0);
        assert_eq!(
            drive(&mut app, key(KeyCode::Enter), t0).await,
            vec![Cmd::Quit]
        );
    }

    /// The alias is a row of its own and resolves to the same command.
    #[tokio::test]
    async fn an_alias_is_offered_and_runs_the_command_it_names() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/exi", t0);
        assert_eq!(menu(&app), vec!["/exit"]);
        assert_eq!(
            drive(&mut app, key(KeyCode::Enter), t0).await,
            vec![Cmd::Quit]
        );
    }

    /// The argument submenu, end to end: past the command token the menu offers that
    /// command's arguments, Tab inserts the selected one, and Enter runs it.
    #[tokio::test]
    async fn the_menu_offers_arguments_once_past_the_command_token() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/help ", t0);
        assert!(app.slash.open, "the argument submenu is up");
        assert!(!app.slash.cursor_in_command);
        assert!(
            menu(&app).contains(&"quit"),
            "every command is offered, named rather than slashed: {:?}",
            menu(&app)
        );

        type_str(&mut app, "qu", t0);
        assert_eq!(menu(&app), vec!["quit"]);
        let _ = app.update(key(KeyCode::Tab), t0);
        assert_eq!(
            app.composer.text(),
            "/help quit",
            "the row inserted its `insert_text`, without the slash"
        );

        let cmds = drive(&mut app, key(KeyCode::Enter), t0).await;
        assert_eq!(cmds, vec![], "a report, not a prompt");
        assert!(
            matches!(app.outbox.last(), Some(Block::Notice(n)) if n.starts_with("/quit ")),
            "{:?}",
            app.outbox.last()
        );
    }

    /// `/model <id>` asks the loop to switch — the reducer never applies it itself,
    /// because rebuilding a provider is IO.
    #[tokio::test]
    async fn switching_the_model_goes_out_as_a_command() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/model claude-opus-5", t0);
        assert_eq!(
            drive(&mut app, key(KeyCode::Enter), t0).await,
            vec![Cmd::SetModel("claude-opus-5".into())]
        );
        assert_eq!(
            app.model.as_deref(),
            Some("mock-1"),
            "the status bar has not moved yet — the engine has not answered"
        );
    }

    /// The status bar follows the model the engine **resolved**, so a refused switch
    /// cannot leave it claiming one the session is not on.
    #[test]
    fn the_status_bar_follows_the_engine_not_the_request() {
        let mut app = ready_app();
        let t0 = Instant::now();

        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::ModelChanged {
                model: "claude-opus-5".into(),
                message: "model: claude-opus-5 (also saved as the default)".into(),
            })),
            t0,
        );
        assert_eq!(app.model.as_deref(), Some("claude-opus-5"));
        assert!(
            matches!(app.outbox.last(), Some(Block::Notice(n)) if n.contains("saved as the default")),
            "{:?}",
            app.outbox.last()
        );

        // A failed switch reports the model still in use.
        let _ = app.update(
            Msg::Engine(Box::new(EngineMsg::ModelChanged {
                model: "claude-opus-5".into(),
                message: "cannot switch to nope: unknown".into(),
            })),
            t0,
        );
        assert_eq!(app.model.as_deref(), Some("claude-opus-5"), "unchanged");
        assert!(
            matches!(app.outbox.last(), Some(Block::Notice(n)) if n.contains("cannot switch")),
            "{:?}",
            app.outbox.last()
        );
    }

    /// Ghost text end to end: typing shows the rest of the name, and Tab — which
    /// accepts the same selected row — makes it real.
    #[tokio::test]
    async fn the_ghost_shows_the_rest_of_the_name_and_tab_accepts_it() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/qu", t0);
        assert_eq!(app.slash.ghost.as_deref(), Some("it"));

        let _ = app.update(key(KeyCode::Tab), t0);
        assert_eq!(app.composer.text(), "/quit");
        assert_eq!(app.slash.ghost, None, "nothing left to complete");
    }

    /// A submitted prompt leaves nothing behind: the emptied composer closes the menu.
    #[test]
    fn submitting_closes_the_menu() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/new", t0);
        assert!(app.slash.open);
        let _ = app.update(key(KeyCode::Enter), t0);
        assert!(!app.slash.open);
        assert!(app.composer.is_empty());
    }

    /// A multiline draft is content, not a command, even when it opens with a slash.
    #[test]
    fn a_multiline_draft_closes_the_menu() {
        let mut app = ready_app();
        let t0 = Instant::now();
        type_str(&mut app, "/new", t0);
        assert!(app.slash.open);
        let _ = app.update(alt_enter(), t0);
        assert!(!app.slash.open, "second line ⇒ ordinary text");
    }
}
