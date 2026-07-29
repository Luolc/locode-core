//! The resumable session trace: one append-only JSONL rollout per session
//! (ADR-0024 §2). Every run leaves `~/.locode/sessions/<encoded-cwd>/`
//! `rollout-<timestamp>-<session_id>.jsonl` — line 1 a `session_meta` header,
//! every other line a `{timestamp, type, payload}` record. The file is a
//! **self-sufficient replay source**: the preamble and every appended message
//! land as `message` lines in append order.
//!
//! Design invariants (the §2.4 extension contract):
//! - the writer only ever *adds* record types / optional header fields;
//! - a torn tail is healed on reopen (a missing final newline gets one before
//!   the next append — grok `jsonl/mod.rs:225-251`), so a crash corrupts at
//!   most one line, which tolerant readers (S4) skip;
//! - dirs `0700`, files `0600`;
//! - tracing failures never kill a run: the writer disables itself on the
//!   first IO error and surfaces it via [`TraceWriter::take_error`].

use std::io::Write;
use std::path::{Path, PathBuf};

use locode_protocol::{Event, Message};
use serde::{Deserialize, Serialize};

use crate::session_dirs::encode_cwd_dirname;

/// The trace schema version (`session_meta.schema_version`). Bumped only by a
/// breaking change the §2.4 rules exist to make unnecessary.
pub const TRACE_SCHEMA_VERSION: u32 = 1;

/// Line-1 payload: the session header (ADR-0024 §2.3). An **open, growing
/// record** — every field beyond the identity core is optional-with-default so
/// old and new binaries share one on-disk population (§2.3 rules).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// The trace schema version.
    pub schema_version: u32,
    /// The session id (also in the filename).
    pub session_id: String,
    /// The session kind — an **open** string: `"main"` today; `"subagent"`/
    /// `"workflow"` later. Listers skip kinds they don't know.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// The parent session id (subagents/forks — §2.4 rule 4). `None` = a root session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// The grouping id (a workflow run — §2.4 rule 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// The canonical start cwd (the directory key — immutable, §2.1).
    pub cwd: PathBuf,
    /// Git context at session start, `None` outside a repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitMeta>,
    /// The CLI version that wrote the trace.
    pub cli_version: String,
    /// The harness pack — recorded so resume rehydrates the right pack
    /// independent of the model catalog (grok's `agent_name` rationale).
    pub harness: String,
    /// The provider wire schema.
    pub api_schema: String,
    /// The model id.
    pub model: String,
}

fn default_kind() -> String {
    "main".to_string()
}

/// Git provenance for the header (grok `summary.json`'s git fields).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitMeta {
    /// The repository root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// The checked-out branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The HEAD commit hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// The `origin` remote URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

/// Construction-time inputs the [`Event::Init`] event doesn't carry.
#[derive(Debug, Clone, Default)]
pub struct TraceExtras {
    /// The CLI version (`env!("CARGO_PKG_VERSION")` at the call site).
    pub cli_version: String,
    /// Git provenance, best-effort.
    pub git: Option<GitMeta>,
    /// The session kind (`"main"` unless spawning a subagent).
    pub kind: Option<String>,
    /// The parent session id, if any.
    pub parent_id: Option<String>,
    /// The workflow/group id, if any.
    pub group: Option<String>,
}

/// The rollout writer: feed it every engine [`Event`]; it materializes the file
/// on `Init` (header + preamble `message` lines) and appends one `message` line
/// per appended [`Message`]. All other event types are ignored — deltas are
/// display-only and the report already has its own artifact (ADR-0014).
#[derive(Debug)]
pub struct TraceWriter {
    sessions_root: PathBuf,
    extras: TraceExtras,
    file: Option<std::fs::File>,
    /// The path of the open rollout (exposed for diagnostics/tests).
    path: Option<PathBuf>,
    /// The first IO error — the writer is disabled once set.
    error: Option<String>,
    /// Resumed writers ignore `Init` (the header/history are already on disk).
    resumed: bool,
}

impl TraceWriter {
    /// A writer that will place rollouts under `sessions_root` (usually
    /// `<locode home>/sessions`). Nothing touches the disk until `Init`.
    #[must_use]
    pub fn new(sessions_root: PathBuf, extras: TraceExtras) -> Self {
        Self {
            sessions_root,
            extras,
            file: None,
            path: None,
            error: None,
            resumed: false,
        }
    }

    /// Reopen an existing rollout for appending (`--continue`/`--resume`,
    /// ADR-0024 §2.5): the torn tail is healed, and the session's `Init`
    /// re-emission is **ignored** — the header and history are already on disk;
    /// only newly appended messages land (codex's reopen-for-append).
    ///
    /// # Errors
    /// When the file cannot be opened for appending.
    pub fn resume(path: PathBuf, sessions_root: PathBuf) -> Result<Self, String> {
        let file = open_append_private(&path)?;
        Ok(Self {
            sessions_root,
            extras: TraceExtras::default(),
            file: Some(file),
            path: Some(path),
            error: None,
            resumed: true,
        })
    }

    /// Feed one engine event. Never fails — on the first IO error the writer
    /// disables itself (fetch the message via [`Self::take_error`]).
    pub fn on_event(&mut self, event: &Event) {
        if self.error.is_some() {
            return;
        }
        let result = match event {
            Event::Init {
                session_id,
                harness,
                api_schema,
                model,
                cwd,
                preamble,
                ..
            } if !self.resumed => {
                self.on_init(session_id, harness, api_schema, model, cwd, preamble)
            }
            Event::Message { message } => self.append_message(message),
            // Per-run usage (additive record type, §2.4): lets resume
            // reconstruct the exact context occupancy instead of estimating.
            //
            // The **final turn's** usage, not the run's sum: this record exists to
            // answer "how full is the context?" on resume, and a sum counts the same
            // conversation once per turn. (Traces written before 2026-07-25 hold the
            // sum here; a session resumed from one over-reports until its first run
            // replaces the number.)
            Event::Result { report } => {
                if self.file.is_some() {
                    self.append_record("usage", &report.context_usage)
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.error = Some(e);
            self.file = None;
        }
    }

    /// The open rollout path (after `Init`).
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The first IO error, if tracing failed (the caller surfaces it as a warning).
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    fn on_init(
        &mut self,
        session_id: &str,
        harness: &str,
        api_schema: &str,
        model: &str,
        cwd: &str,
        preamble: &[Message],
    ) -> Result<(), String> {
        let cwd_path = PathBuf::from(cwd);
        let dirname = encode_cwd_dirname(&cwd_path);
        let dir = self.sessions_root.join(&dirname);
        create_dir_private(&dir)?;
        // Hash-fallback dirs get the `.cwd` sidecar recovering the original path
        // (§2.1 — fallback names never start with `+`).
        if !dirname.starts_with('+') {
            let sidecar = dir.join(".cwd");
            if !sidecar.exists() {
                std::fs::write(&sidecar, cwd).map_err(|e| format!("write .cwd sidecar: {e}"))?;
            }
        }
        let filename = format!("rollout-{}-{session_id}.jsonl", filename_timestamp());
        let path = dir.join(&filename);
        let file = open_append_private(&path)?;
        self.file = Some(file);
        self.path = Some(path);

        let meta = SessionMeta {
            schema_version: TRACE_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            kind: self
                .extras
                .kind
                .clone()
                .unwrap_or_else(|| "main".to_string()),
            parent_id: self.extras.parent_id.clone(),
            group: self.extras.group.clone(),
            cwd: cwd_path,
            git: self.extras.git.clone(),
            cli_version: self.extras.cli_version.clone(),
            harness: harness.to_string(),
            api_schema: api_schema.to_string(),
            model: model.to_string(),
        };
        self.append_record("session_meta", &meta)?;
        // The preamble rides as ordinary message lines — the file replays alone.
        for message in preamble {
            self.append_record("message", message)?;
        }
        Ok(())
    }

    fn append_message(&mut self, message: &Message) -> Result<(), String> {
        if self.file.is_none() {
            return Ok(()); // no Init seen (tracing effectively off)
        }
        self.append_record("message", message)
    }

    fn append_record(&mut self, record_type: &str, payload: &impl Serialize) -> Result<(), String> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        let line = serde_json::json!({
            "timestamp": now_rfc3339_millis(),
            "type": record_type,
            "payload": payload,
        });
        let mut buf = serde_json::to_string(&line).map_err(|e| format!("serialize: {e}"))?;
        buf.push('\n');
        file.write_all(buf.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|e| format!("append trace record: {e}"))
    }
}

/// A parsed rollout: the header plus the replayable message history.
#[derive(Debug)]
pub struct RolloutContents {
    /// The line-1 header.
    pub meta: SessionMeta,
    /// The conversation, in append order (preamble included) — `compacted`
    /// records already folded in.
    pub history: Vec<Message>,
    /// The **last** run's provider-reported usage, when the rollout carries
    /// `usage` records (written since 2026-07-24) — the exact basis for the
    /// resumed context occupancy. `None` on older rollouts (callers estimate).
    pub last_usage: Option<locode_protocol::Usage>,
}

/// Read a rollout **tolerantly** (ADR-0024 §2.4 rules 1-2): unknown record
/// types and unknown payload fields are skipped/ignored; an unparsable line
/// (the torn tail) is skipped; a `compacted` record replaces all prior
/// messages with its `replacement_history`.
///
/// # Errors
/// Only when the file is unreadable or line 1 is not a valid `session_meta`
/// (without a header the file identifies nothing).
pub fn read_rollout(path: &Path) -> Result<RolloutContents, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut lines = text.lines();
    let first = lines
        .next()
        .ok_or_else(|| format!("{}: empty rollout", path.display()))?;
    let first: serde_json::Value = serde_json::from_str(first)
        .map_err(|e| format!("{}: line 1 unparsable: {e}", path.display()))?;
    if first.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
        return Err(format!("{}: line 1 is not session_meta", path.display()));
    }
    let meta: SessionMeta = serde_json::from_value(first["payload"].clone())
        .map_err(|e| format!("{}: session_meta invalid: {e}", path.display()))?;

    let mut history: Vec<Message> = Vec::new();
    let mut last_usage: Option<locode_protocol::Usage> = None;
    for line in lines {
        // Torn tail / foreign garbage: skip, never fail (§2.4).
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("message") => {
                if let Ok(message) = serde_json::from_value::<Message>(value["payload"].clone()) {
                    history.push(message);
                }
            }
            Some("compacted") => {
                // Fold: the replacement history stands in for everything prior.
                if let Some(replacement) = value["payload"].get("replacement_history")
                    && let Ok(messages) =
                        serde_json::from_value::<Vec<Message>>(replacement.clone())
                {
                    history = messages;
                }
            }
            Some("usage") => {
                if let Ok(usage) =
                    serde_json::from_value::<locode_protocol::Usage>(value["payload"].clone())
                {
                    last_usage = Some(usage);
                }
            }
            // Unknown record types (future features) are invisible to this reader.
            _ => {}
        }
    }
    Ok(RolloutContents {
        meta,
        history,
        last_usage,
    })
}

/// The newest resumable rollout for `cwd` (`--continue`): list the one encoded
/// directory, take rollouts newest-first by filename (the timestamp prefix
/// sorts chronologically), and return the first whose header parses with
/// `kind == "main"` — unknown kinds are not listed (§2.4 rule 3), but remain
/// reachable by id.
#[must_use]
pub fn find_latest_rollout(sessions_root: &Path, cwd: &Path) -> Option<PathBuf> {
    let dir = sessions_root.join(encode_cwd_dirname(cwd));
    let mut names: Vec<String> = rollout_names(&dir);
    names.sort_unstable_by(|a, b| b.cmp(a)); // newest first
    names
        .into_iter()
        .map(|n| dir.join(n))
        .find(|path| read_rollout(path).is_ok_and(|c| c.meta.kind == "main"))
}

/// Locate a rollout by session id (`--resume <id>`): the cwd's directory first,
/// then every other session directory (Claude's scoped-then-global resolver).
/// Any `kind` is resumable by id.
#[must_use]
pub fn find_rollout_by_id(sessions_root: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("-{id}.jsonl");
    let scoped = sessions_root.join(encode_cwd_dirname(cwd));
    if let Some(hit) = dir_hit(&scoped, &suffix) {
        return Some(hit);
    }
    let entries = std::fs::read_dir(sessions_root).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir == scoped || !dir.is_dir() {
            continue;
        }
        if let Some(hit) = dir_hit(&dir, &suffix) {
            return Some(hit);
        }
    }
    None
}

/// One row of the session picker (ADR-0029): everything a chooser needs, read
/// from the rollout's **line-1 header plus its mtime** — never the whole file.
///
/// The conversation title is deliberately absent: it lives in the first user
/// message, which costs a second read of a few records, so the picker paints from
/// these and fills titles in afterwards (codex's lazy-preview pattern,
/// `tui/src/resume_picker.rs:772`, minus the preview pane).
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// The session id, as it appears in the filename and the header.
    pub id: String,
    /// The rollout to resume.
    pub path: PathBuf,
    /// The session's start cwd (the directory key).
    pub cwd: PathBuf,
    /// The harness pack that wrote it. Rows are **not** filtered by it: resuming
    /// switches the pack from here, so a session started under another harness is
    /// still listed and still resumable (ADR-0029 resolution 1).
    pub harness: String,
    /// The model at session start.
    pub model: String,
    /// The git branch at session start, when the session began inside a repo.
    pub branch: Option<String>,
    /// File mtime — the session's last activity, and the sort key.
    pub last_active: std::time::SystemTime,
}

/// Which sessions [`list_sessions`] returns.
#[derive(Debug, Clone, Copy)]
pub enum SessionScope<'a> {
    /// Only sessions started in this directory (the picker's default).
    Cwd(&'a Path),
    /// Every session under the root, across directories.
    All,
}

/// List resumable sessions, newest activity first (ADR-0029).
///
/// Reads **line 1 and the mtime** of each `rollout-*.jsonl`, so the cost is one
/// open per session rather than one parse of every transcript. ADR-0024 reserved a
/// listing index for this; it stays reserved, because a directory scan cannot
/// disagree with the directory while an index can — revisit on a measurement, not
/// a guess.
///
/// Skips, silently and by design:
/// - rollouts whose header will not parse (ADR-0024 §2.4 tolerance — a torn file is
///   invisible in the picker, not an error row the user must dismiss);
/// - `kind != "main"`, so subagent and workflow transcripts never appear as
///   something to resume into (ADR-0029 resolution 2). They stay reachable by id.
#[must_use]
pub fn list_sessions(sessions_root: &Path, scope: SessionScope<'_>) -> Vec<SessionSummary> {
    let dirs: Vec<PathBuf> = match scope {
        SessionScope::Cwd(cwd) => vec![sessions_root.join(encode_cwd_dirname(cwd))],
        SessionScope::All => std::fs::read_dir(sessions_root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
    };
    let mut out: Vec<SessionSummary> = dirs
        .iter()
        .flat_map(|dir| {
            rollout_names(dir)
                .into_iter()
                .filter_map(|name| summarize_rollout(&dir.join(name)))
        })
        .collect();
    // Newest activity first. Ties keep a stable order so a redraw cannot reshuffle
    // rows under the cursor.
    out.sort_by(|a, b| b.last_active.cmp(&a.last_active).then(a.id.cmp(&b.id)));
    out
}

/// The one-line title for a picker row: the first **real** user message in the
/// rollout, first line only.
///
/// Not in the header, so it costs a second read — which is why the picker fills
/// titles in for the rows it is about to draw rather than for the whole list
/// (ADR-0029; codex loads previews the same way,
/// `tui/src/resume_picker.rs:772`). Reads at most `TITLE_SCAN_LINES` records, so
/// a session that opens with a long injected preamble does not turn into a full
/// parse.
///
/// Injected framing is skipped: an `AGENTS.md` reminder or a skills listing is a
/// `User` message the user never typed, and titling a session with it would make
/// every session in a project look identical.
#[must_use]
pub fn read_session_title(path: &Path) -> Option<String> {
    use std::io::BufRead as _;
    let file = std::fs::File::open(path).ok()?;
    for line in std::io::BufReader::new(file).lines().take(TITLE_SCAN_LINES) {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("message") {
            continue;
        }
        let Ok(message) = serde_json::from_value::<Message>(value.get("payload")?.clone()) else {
            continue;
        };
        if message.role != locode_protocol::Role::User {
            continue;
        }
        let text: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                locode_protocol::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        let first_line = text.lines().find(|l| !l.trim().is_empty())?.trim();
        if first_line.starts_with("<system-reminder>") {
            continue;
        }
        return Some(first_line.to_string());
    }
    None
}

/// How far into a rollout [`read_session_title`] looks for the first user
/// message before giving up — a bound, so one pathological file cannot stall a
/// paint.
const TITLE_SCAN_LINES: usize = 64;

/// Read one rollout's header + mtime into a summary, or `None` if it is not a
/// listable session.
fn summarize_rollout(path: &Path) -> Option<SessionSummary> {
    let file = std::fs::File::open(path).ok()?;
    let last_active = file.metadata().ok()?.modified().ok()?;
    let mut first = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(file), &mut first).ok()?;
    let value: serde_json::Value = serde_json::from_str(first.trim_end()).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
        return None;
    }
    let meta: SessionMeta = serde_json::from_value(value.get("payload")?.clone()).ok()?;
    if meta.kind != "main" {
        return None;
    }
    Some(SessionSummary {
        id: meta.session_id,
        path: path.to_path_buf(),
        cwd: meta.cwd,
        harness: meta.harness,
        model: meta.model,
        branch: meta.git.and_then(|g| g.branch),
        last_active,
    })
}

/// The rollout filenames directly inside `dir`.
fn rollout_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            n.starts_with("rollout-")
                && std::path::Path::new(n)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
        })
        .collect()
}

fn dir_hit(dir: &Path, suffix: &str) -> Option<PathBuf> {
    rollout_names(dir)
        .into_iter()
        .find(|n| n.ends_with(suffix))
        .map(|n| dir.join(n))
}

/// Open (or create) a rollout for appending, `0600`, healing a torn tail: if the
/// last byte isn't `\n`, one is appended first so the next record starts clean —
/// bounding crash damage to exactly one line (grok's rule).
pub(crate) fn open_append_private(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    heal_torn_tail(path, &mut file)?;
    Ok(file)
}

fn heal_torn_tail(path: &Path, file: &mut std::fs::File) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let len = file
        .metadata()
        .map_err(|e| format!("stat {}: {e}", path.display()))?
        .len();
    if len == 0 {
        return Ok(());
    }
    // Read the final byte via a fresh read handle (`self` is append-only).
    let mut reader =
        std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    reader
        .seek(SeekFrom::End(-1))
        .map_err(|e| format!("seek {}: {e}", path.display()))?;
    let mut last = [0_u8; 1];
    reader
        .read_exact(&mut last)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if last[0] != b'\n' {
        file.write_all(b"\n")
            .map_err(|e| format!("heal {}: {e}", path.display()))?;
    }
    Ok(())
}

/// `mkdir -p` with `0700` on every newly created component (best-effort mode —
/// pre-existing dirs are left alone).
pub(crate) fn create_dir_private(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("create {}: {e}", dir.display()))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))
    }
}

/// The record timestamp: RFC3339 with milliseconds, UTC (codex's shape).
fn now_rfc3339_millis() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// The filename timestamp: colons → dashes for filesystem compatibility
/// (codex `recorder.rs:1547-1550`); reverse-chron name-sorting within a dir.
fn filename_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use locode_protocol::{ContentBlock, Role};
    use serde_json::Value;

    fn init_event(session_id: &str, cwd: &Path) -> Event {
        Event::Init {
            session_id: session_id.to_string(),
            harness: "codex".to_string(),
            api_schema: "mock".to_string(),
            model: "mock-1".to_string(),
            cwd: cwd.to_string_lossy().into_owned(),
            max_turns: None,
            preamble: vec![Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "base prompt".to_string(),
                }],
            }],
            tools: vec![],
        }
    }

    fn message_event(role: Role, text: &str) -> Event {
        Event::Message {
            message: Message {
                role,
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            },
        }
    }

    fn read_lines(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn writes_header_preamble_and_messages() {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(cwd.path()).unwrap();
        let mut writer = TraceWriter::new(
            root.path().join("sessions"),
            TraceExtras {
                cli_version: "0.1.9".to_string(),
                git: Some(GitMeta {
                    branch: Some("main".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        writer.on_event(&init_event("sess-1", &cwd));
        writer.on_event(&message_event(Role::User, "hi"));
        writer.on_event(&Event::MessageDelta {
            text: "ignored".to_string(),
        });
        writer.on_event(&message_event(Role::Assistant, "hello"));
        assert!(writer.take_error().is_none());

        // The file landed in the encoded-cwd dir with the id in the name.
        let path = writer.path().unwrap().to_path_buf();
        assert_eq!(
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            encode_cwd_dirname(&cwd)
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("rollout-")
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("-sess-1.jsonl")
        );

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 4, "meta + preamble + 2 messages");
        assert_eq!(lines[0]["type"], "session_meta");
        let meta = &lines[0]["payload"];
        assert_eq!(meta["schema_version"], 1);
        assert_eq!(meta["session_id"], "sess-1");
        assert_eq!(meta["kind"], "main");
        assert_eq!(meta["harness"], "codex");
        assert_eq!(meta["git"]["branch"], "main");
        assert!(meta.get("parent_id").is_none(), "absent when None");
        // Preamble + appended messages are verbatim protocol Messages.
        assert_eq!(lines[1]["type"], "message");
        assert_eq!(lines[1]["payload"]["role"], "system");
        assert_eq!(lines[2]["payload"]["role"], "user");
        assert_eq!(lines[3]["payload"]["role"], "assistant");
        // Every line carries a timestamp.
        for line in &lines {
            assert!(line["timestamp"].as_str().unwrap().ends_with('Z'));
        }
    }

    #[cfg(unix)]
    #[test]
    fn modes_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(cwd.path()).unwrap();
        let mut writer = TraceWriter::new(root.path().join("sessions"), TraceExtras::default());
        writer.on_event(&init_event("sess-2", &cwd));
        let path = writer.path().unwrap();
        let file_mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "rollout is 0600");
        assert_eq!(dir_mode, 0o700, "session dir is 0700");
    }

    #[test]
    fn torn_tail_is_healed_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-x.jsonl");
        std::fs::write(&path, "{\"ok\":1}\n{\"torn\":").unwrap();
        let mut file = open_append_private(&path).unwrap();
        file.write_all(b"{\"next\":2}\n").unwrap();
        drop(file);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "{\"ok\":1}\n{\"torn\":\n{\"next\":2}\n");
        // Exactly one line is garbage; the rest parse.
        let parsed: Vec<_> = text
            .lines()
            .filter(|l| serde_json::from_str::<Value>(l).is_ok())
            .collect();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn no_init_means_no_file_and_errors_disable() {
        let root = tempfile::tempdir().unwrap();
        let mut writer = TraceWriter::new(root.path().join("sessions"), TraceExtras::default());
        writer.on_event(&message_event(Role::User, "before init"));
        assert!(writer.path().is_none(), "nothing written before Init");

        // An unwritable root disables the writer with an error, never a panic.
        let mut writer = TraceWriter::new(PathBuf::from("/dev/null/nope"), TraceExtras::default());
        let cwd = std::env::temp_dir();
        writer.on_event(&init_event("sess-3", &cwd));
        assert!(writer.take_error().is_some());
        // Further events are no-ops.
        writer.on_event(&message_event(Role::User, "x"));
    }

    /// Write a rollout by hand so a test can control the header exactly (kinds and
    /// torn files the writer would never produce).
    fn write_rollout(dir: &Path, id: &str, kind: &str, extra_lines: &[&str]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("rollout-2026-07-29T00-00-00-{id}.jsonl"));
        let meta = serde_json::json!({
            "timestamp": "2026-07-29T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "schema_version": 1,
                "session_id": id,
                "kind": kind,
                "cwd": dir.to_string_lossy(),
                "git": { "branch": "main" },
                "cli_version": "0.1.17",
                "harness": "claude",
                "api_schema": "anthropic",
                "model": "test-model",
            }
        });
        let mut body = meta.to_string();
        for line in extra_lines {
            body.push('\n');
            body.push_str(line);
        }
        body.push('\n');
        std::fs::write(&path, body).unwrap();
        path
    }

    fn set_mtime(path: &Path, when: std::time::SystemTime) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    /// The picker's listing (ADR-0029): newest activity first, scoped to one cwd by
    /// default, and quietly skipping what is not a resumable main session.
    #[test]
    fn list_sessions_orders_scopes_and_skips() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        let cwd_a = PathBuf::from("/tmp/project-a");
        let cwd_b = PathBuf::from("/tmp/project-b");
        let dir_a = sessions.join(encode_cwd_dirname(&cwd_a));
        let dir_b = sessions.join(encode_cwd_dirname(&cwd_b));

        let older = write_rollout(&dir_a, "sess-old", "main", &[]);
        let newer = write_rollout(&dir_a, "sess-new", "main", &[]);
        write_rollout(&dir_a, "sess-sub", "subagent", &[]); // not resumable-into
        write_rollout(&dir_b, "sess-other-dir", "main", &[]);

        // Distinct mtimes, oldest first, so ordering is not filename luck (and no
        // sleeping: `File::set_modified` is std, no dependency).
        let base = std::time::SystemTime::now() - std::time::Duration::from_mins(10);
        set_mtime(&older, base);
        set_mtime(&newer, base + std::time::Duration::from_mins(5));

        // Unlistable rollouts: empty, non-JSON line 1, and a line 1 that is not a header.
        std::fs::write(dir_a.join("rollout-2026-07-29T00-00-00-empty.jsonl"), "").unwrap();
        std::fs::write(
            dir_a.join("rollout-2026-07-29T00-00-00-garbage.jsonl"),
            "not json\n",
        )
        .unwrap();
        std::fs::write(
            dir_a.join("rollout-2026-07-29T00-00-00-nohdr.jsonl"),
            "{\"type\":\"message\",\"payload\":{}}\n",
        )
        .unwrap();

        let scoped = list_sessions(&sessions, SessionScope::Cwd(&cwd_a));
        let ids: Vec<&str> = scoped.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["sess-new", "sess-old"],
            "newest first; subagent, torn, and headerless rollouts are invisible"
        );
        assert_eq!(scoped[0].harness, "claude", "header fields survive");
        assert_eq!(scoped[0].branch.as_deref(), Some("main"));
        assert_eq!(scoped[0].model, "test-model");

        let all = list_sessions(&sessions, SessionScope::All);
        assert!(
            all.iter().any(|s| s.id == "sess-other-dir"),
            "All crosses directories: {:?}",
            all.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
        assert_eq!(all.len(), 3, "still no subagent or torn rows: {all:?}");
    }

    /// A missing sessions root is an empty picker, not an error — a first run has
    /// no directory at all.
    #[test]
    fn list_sessions_on_a_missing_root_is_empty() {
        let root = tempfile::tempdir().unwrap();
        let absent = root.path().join("nope");
        assert!(list_sessions(&absent, SessionScope::All).is_empty());
        assert!(list_sessions(&absent, SessionScope::Cwd(Path::new("/tmp/x"))).is_empty());
    }

    #[test]
    fn read_rollout_replays_and_tolerates() {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(cwd.path()).unwrap();
        let mut writer = TraceWriter::new(root.path().join("sessions"), TraceExtras::default());
        writer.on_event(&init_event("sess-r", &cwd));
        writer.on_event(&message_event(Role::User, "hi"));
        writer.on_event(&message_event(Role::Assistant, "yo"));
        let path = writer.path().unwrap().to_path_buf();

        // Sprinkle §2.4 hazards: an unknown record type, a torn tail.
        {
            let mut file = open_append_private(&path).unwrap();
            file.write_all(
                b"{\"timestamp\":\"x\",\"type\":\"future_thing\",\"payload\":{\"n\":1}}\n",
            )
            .unwrap();
            file.write_all(b"{\"torn").unwrap();
        }
        let contents = read_rollout(&path).unwrap();
        assert_eq!(contents.meta.session_id, "sess-r");
        // preamble(1) + user + assistant; the unknown type and torn line vanish.
        assert_eq!(contents.history.len(), 3);
        assert_eq!(contents.history[0].role, Role::System);
        assert_eq!(contents.history[2].role, Role::Assistant);
    }

    #[test]
    fn read_rollout_folds_compacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-t-sess-c.jsonl");
        let meta = serde_json::json!({"timestamp":"t","type":"session_meta","payload":{
            "schema_version":1,"session_id":"sess-c","kind":"main","cwd":"/x",
            "cli_version":"0","harness":"grok","api_schema":"mock","model":"m"}});
        let msg = |role: &str, text: &str| {
            serde_json::json!({"timestamp":"t","type":"message","payload":
                {"role":role,"content":[{"type":"text","text":text}]}})
        };
        let compacted = serde_json::json!({"timestamp":"t","type":"compacted","payload":{
            "summary":"s","replacement_history":[
                {"role":"user","content":[{"type":"text","text":"summary of the past"}]}]}});
        let lines = [
            meta.to_string(),
            msg("user", "old-1").to_string(),
            msg("assistant", "old-2").to_string(),
            compacted.to_string(),
            msg("user", "after").to_string(),
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let contents = read_rollout(&path).unwrap();
        // Compaction replaced the two old messages; the post-compaction one follows.
        assert_eq!(contents.history.len(), 2);
        assert_eq!(contents.history[1].role, Role::User);
    }

    #[test]
    fn resumed_writer_appends_without_a_new_header() {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(cwd.path()).unwrap();
        let mut writer = TraceWriter::new(root.path().join("sessions"), TraceExtras::default());
        writer.on_event(&init_event("sess-a", &cwd));
        writer.on_event(&message_event(Role::User, "first run"));
        let path = writer.path().unwrap().to_path_buf();
        let before = read_lines(&path).len();
        drop(writer);

        let mut resumed = TraceWriter::resume(path.clone(), root.path().join("sessions")).unwrap();
        resumed.on_event(&init_event("sess-a", &cwd)); // re-Init: ignored
        resumed.on_event(&message_event(Role::User, "second run"));
        assert!(resumed.take_error().is_none());
        let lines = read_lines(&path);
        assert_eq!(lines.len(), before + 1, "exactly one new message line");
        assert_eq!(
            lines.iter().filter(|l| l["type"] == "session_meta").count(),
            1,
            "one header, ever"
        );
    }

    #[test]
    fn find_latest_scopes_by_cwd_and_kind() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        let cwd_a = tempfile::tempdir().unwrap();
        let cwd_a = std::fs::canonicalize(cwd_a.path()).unwrap();
        let cwd_b = tempfile::tempdir().unwrap();
        let cwd_b = std::fs::canonicalize(cwd_b.path()).unwrap();

        // Older main session in A; then a subagent session in A; a session in B.
        let mut w1 = TraceWriter::new(sessions.clone(), TraceExtras::default());
        w1.on_event(&init_event("sess-old", &cwd_a));
        let mut w2 = TraceWriter::new(
            sessions.clone(),
            TraceExtras {
                kind: Some("subagent".to_string()),
                ..Default::default()
            },
        );
        w2.on_event(&init_event("sess-sub", &cwd_a));
        let mut w3 = TraceWriter::new(sessions.clone(), TraceExtras::default());
        w3.on_event(&init_event("sess-b", &cwd_b));

        // Continue in A: the subagent is skipped; the old main session wins.
        let latest = find_latest_rollout(&sessions, &cwd_a).unwrap();
        assert!(latest.to_string_lossy().contains("sess-old"), "{latest:?}");
        // Continue in B is scoped to B.
        let latest_b = find_latest_rollout(&sessions, &cwd_b).unwrap();
        assert!(latest_b.to_string_lossy().contains("sess-b"));
        // A cwd with no sessions has nothing to continue.
        let empty = tempfile::tempdir().unwrap();
        assert!(find_latest_rollout(&sessions, empty.path()).is_none());

        // Resume by id: found from the "wrong" cwd via the global scan — and a
        // subagent IS resumable by id.
        let by_id = find_rollout_by_id(&sessions, &cwd_b, "sess-sub").unwrap();
        assert!(by_id.to_string_lossy().contains("sess-sub"));
        assert!(find_rollout_by_id(&sessions, &cwd_b, "sess-nope").is_none());
    }

    #[test]
    fn usage_records_round_trip_for_exact_resume() {
        use locode_protocol::{Report, Status, Usage};
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(cwd.path()).unwrap();
        let mut writer = TraceWriter::new(root.path().join("sessions"), TraceExtras::default());
        writer.on_event(&init_event("sess-u", &cwd));
        writer.on_event(&message_event(Role::User, "hi"));
        // Two runs: the LAST usage wins.
        let report = |input: u64, output: u64| Report {
            schema_version: 1,
            status: Status::Completed,
            harness: "grok".into(),
            api_schema: "mock".into(),
            final_message: None,
            structured_output: None,
            turns: 1,
            tool_calls: vec![],
            // The run's sum — deliberately different from `context_usage`, so the test
            // proves which one the record carries.
            usage: Usage {
                input_tokens: input * 3,
                output_tokens: output * 3,
                cache_read_tokens: Some(99),
                ..Default::default()
            },
            context_usage: Usage {
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: Some(7),
                cache_creation_tokens: Some(5),
                ..Default::default()
            },
            session_id: "sess-u".into(),
            stop_reason: None,
            error: None,
        };
        writer.on_event(&locode_protocol::Event::Result {
            report: report(100, 10),
        });
        writer.on_event(&locode_protocol::Event::Result {
            report: report(200, 20),
        });
        assert!(writer.take_error().is_none());

        let contents = read_rollout(writer.path().unwrap()).unwrap();
        let usage = contents.last_usage.expect("usage recovered");
        assert_eq!(usage.input_tokens, 200, "the last run's context usage");
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, Some(7));
        // The record is the context occupancy, NOT the run's cost total: resuming from
        // the sum would over-report a long run's context by a multiple.
        assert_eq!(
            usage.context_tokens(),
            200 + 7 + 5 + 20,
            "both cache counters are part of the prompt"
        );
        // And an old-style rollout (no usage records) still reads fine.
        let bare = root.path().join("bare.jsonl");
        let meta_line = std::fs::read_to_string(writer.path().unwrap())
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        std::fs::write(&bare, meta_line + "\n").unwrap();
        assert!(read_rollout(&bare).unwrap().last_usage.is_none());
    }

    #[test]
    fn reserved_kinds_and_parents_serialize() {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(cwd.path()).unwrap();
        let mut writer = TraceWriter::new(
            root.path().join("sessions"),
            TraceExtras {
                cli_version: "x".to_string(),
                kind: Some("subagent".to_string()),
                parent_id: Some("sess-parent".to_string()),
                group: Some("wf-1".to_string()),
                ..Default::default()
            },
        );
        writer.on_event(&init_event("sess-4", &cwd));
        let lines = read_lines(writer.path().unwrap());
        let meta = &lines[0]["payload"];
        assert_eq!(meta["kind"], "subagent");
        assert_eq!(meta["parent_id"], "sess-parent");
        assert_eq!(meta["group"], "wf-1");
        // And the meta round-trips through the typed struct (forward-compat:
        // unknown fields would be ignored by this same path).
        let parsed: SessionMeta = serde_json::from_value(meta.clone()).unwrap();
        assert_eq!(parsed.kind, "subagent");
    }
}
