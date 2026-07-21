//! Shell execution: capture stdout/stderr/exit, hard timeout, output cap, group-kill.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::Host;

/// A command to run through the host's shell.
#[derive(Debug, Clone)]
pub struct ExecRequest {
    /// The command line, run as `<shell> -lc <command>` (or `-c` when not a login shell).
    pub command: String,
    /// Working directory (the caller jail-resolves this).
    pub cwd: PathBuf,
    /// Per-call timeout; `None` uses the host default, then clamped to the host max.
    pub timeout: Option<Duration>,
    /// Extra environment variables, layered on top of the inherited environment.
    pub env: Vec<(String, String)>,
    /// Harness-specific shell shape (Task 26 Slice 0b); `None` = host default.
    pub shell: Option<ShellSpec>,
    /// Opt-in combined front/back capture + spill; `None` = per-stream tails.
    pub front_back: Option<FrontBackSpec>,
}

/// A harness's shell invocation shape (plan-finalization Q5, 2026-07-21): the
/// *choice* rides the request; execution stays behind the host door. Grok's
/// shape is `{ detect_program: true, login_arg: false, login_path_probe: true }`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShellSpec {
    /// Use the user's `$SHELL` when its basename is `bash` or `zsh` (grok's
    /// detection); otherwise fall back to the host's configured program.
    pub detect_program: bool,
    /// `true` = `-lc` (login), `false` = `-c` (grok always uses `-c`).
    pub login_arg: bool,
    /// Inject the login shell's `PATH` (probed once per host, cached) so
    /// non-login `-c` children still see user PATH entries (grok's probe).
    pub login_path_probe: bool,
}

/// Opt-in combined-stream retention (Task 26 Slice 0b; grok's bash capture
/// shape): keep the first and last halves of a char budget, stream the full
/// output to a host-owned spill file. The pack renders separators/headers —
/// the host returns structure only.
#[derive(Debug, Clone, Copy)]
pub struct FrontBackSpec {
    /// Total retained characters across front + back (grok: `20_000`).
    pub char_budget: usize,
    /// Also stream the full combined output to a temp spill file
    /// (retention-capped at [`SPILL_RETAIN_MAX`]).
    pub spill: bool,
}

/// Hard cap on bytes written to a spill file (grok retains ≤ 64 MiB).
pub const SPILL_RETAIN_MAX: u64 = 64 * 1024 * 1024;

/// The combined front/back capture (present when the request asked for it).
#[derive(Debug, Clone)]
pub struct FrontBackCapture {
    /// The first ~half of the char budget (everything, when not truncated).
    pub front: String,
    /// The last ~half of the char budget; `None` when nothing was cut.
    pub back: Option<String>,
    /// True total combined output size in bytes (before any retention).
    pub total_bytes: u64,
    /// The spill file holding the full output (≤ [`SPILL_RETAIN_MAX`]), when
    /// requested and creatable.
    pub spill_path: Option<PathBuf>,
}

/// The captured result of a command (completed, timed out, or cancelled).
#[derive(Debug, Clone)]
pub struct ExecOutput {
    /// Captured stdout (tail-retained to the byte cap, lossy UTF-8).
    pub stdout: String,
    /// Captured stderr (tail-retained to the byte cap, lossy UTF-8).
    pub stderr: String,
    /// stdout then stderr, the convenience most tools render.
    pub combined: String,
    /// The exit code, or `None` if the command was killed by a signal.
    pub exit_code: Option<i32>,
    /// Whether the hard timeout fired.
    pub timed_out: bool,
    /// Whether cooperative cancellation fired.
    pub cancelled: bool,
    /// Whether either stream hit the byte cap (older bytes dropped).
    pub truncated: bool,
    /// Wall-clock duration.
    pub duration: Duration,
    /// Combined front/back capture (only when the request set `front_back`).
    pub front_back: Option<FrontBackCapture>,
}

/// A failure to *spawn or capture* — not a failed command (that is a successful capture
/// with a non-zero `exit_code`).
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    /// The shell process could not be spawned.
    #[error("failed to spawn shell: {0}")]
    Spawn(String),
}

impl Host {
    /// Run `req` through the shell, capturing output under the host's limits and honoring
    /// cooperative `cancel`.
    ///
    /// A command that fails, times out, or is cancelled is a **successful capture** with
    /// the corresponding fields set — the pack decides how to present it.
    ///
    /// # Errors
    /// [`ExecError::Spawn`] only if the shell process itself cannot be started.
    pub async fn exec(
        &self,
        req: ExecRequest,
        cancel: &CancellationToken,
    ) -> Result<ExecOutput, ExecError> {
        let timeout = req
            .timeout
            .unwrap_or(self.limits.default_timeout)
            .min(self.limits.max_timeout);
        let program = self.shell_program_for(req.shell);
        let path_env = match req.shell {
            Some(spec) if spec.login_path_probe => self.probed_login_path(&program).await,
            _ => None,
        };
        let cmd = self.build_shell_command(&req, &program, path_env.as_deref());
        self.run_captured(cmd, timeout, cancel, req.front_back)
            .await
    }

    /// The shell program for a request: `$SHELL` when detection is asked for
    /// and its basename is `bash`/`zsh` (grok's rule), else the host default.
    fn shell_program_for(&self, spec: Option<ShellSpec>) -> String {
        if spec.is_some_and(|s| s.detect_program)
            && let Ok(user_shell) = std::env::var("SHELL")
        {
            let base = std::path::Path::new(&user_shell)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            if matches!(base.as_deref(), Some("bash" | "zsh")) {
                return user_shell;
            }
        }
        self.shell_program.clone()
    }

    /// The login shell's `PATH`, probed once per host and cached (grok's
    /// login-PATH probe): `<program> -lc 'printf %s "$PATH"'` with a short
    /// timeout; `None` (no injection) when the probe fails.
    async fn probed_login_path(&self, program: &str) -> Option<String> {
        self.login_path
            .get_or_init(|| async {
                let probe = Command::new(program)
                    .arg("-lc")
                    .arg("printf %s \"$PATH\"")
                    .stdin(Stdio::null())
                    .output();
                match tokio::time::timeout(Duration::from_secs(5), probe).await {
                    Ok(Ok(out)) if out.status.success() => {
                        let path = String::from_utf8_lossy(&out.stdout).into_owned();
                        (!path.trim().is_empty()).then_some(path)
                    }
                    _ => None,
                }
            })
            .await
            .clone()
    }

    /// Run a resolved program with explicit argv (**no shell** — safe for untrusted args
    /// like a regex or glob), capturing output under the same limits/kill/cancel machinery
    /// as [`Host::exec`]. Used by rg-backed tools (Task 11).
    ///
    /// # Errors
    /// [`ExecError::Spawn`] if the program cannot be started (e.g. `rg` not on PATH).
    pub async fn run_capture(
        &self,
        program: &Path,
        args: &[String],
        cwd: &Path,
        timeout: Option<Duration>,
        cancel: &CancellationToken,
    ) -> Result<ExecOutput, ExecError> {
        let timeout = timeout
            .unwrap_or(self.limits.default_timeout)
            .min(self.limits.max_timeout);
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.current_dir(cwd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        self.run_captured(cmd, timeout, cancel, None).await
    }

    /// The shared spawn → bounded-capture → timeout/cancel-kill → assemble body.
    async fn run_captured(
        &self,
        mut cmd: Command,
        timeout: Duration,
        cancel: &CancellationToken,
        front_back: Option<FrontBackSpec>,
    ) -> Result<ExecOutput, ExecError> {
        let started = Instant::now();
        let cap = self.limits.max_output_bytes;

        let mut child = cmd.spawn().map_err(|e| ExecError::Spawn(e.to_string()))?;
        let pid = child.id();

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (out_task, err_task, combined_task) = if let Some(spec) = front_back {
            // Combined interleaved capture: both streams forward chunks in
            // arrival order to one collector (front/back retention + spill);
            // per-stream capped buffers are maintained alongside for the
            // existing `stdout`/`stderr` fields.
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let tx_err = tx.clone();
            let out = tokio::spawn(read_capped_forwarding(stdout, cap, tx));
            let err = tokio::spawn(read_capped_forwarding(stderr, cap, tx_err));
            let collector = tokio::spawn(collect_front_back(rx, spec));
            (out, err, Some(collector))
        } else {
            (
                tokio::spawn(read_capped(stdout, cap)),
                tokio::spawn(read_capped(stderr, cap)),
                None,
            )
        };

        let mut exit_code = None;
        let mut timed_out = false;
        let mut cancelled = false;
        tokio::select! {
            status = child.wait() => {
                exit_code = status.ok().and_then(|s| s.code());
            }
            () = tokio::time::sleep(timeout) => {
                timed_out = true;
                terminate(&mut child, pid, self.limits.kill_grace).await;
            }
            () = cancel.cancelled() => {
                cancelled = true;
                terminate(&mut child, pid, self.limits.kill_grace).await;
            }
        }

        let (stdout, out_trunc) = out_task.await.unwrap_or_else(|_| (Vec::new(), false));
        let (stderr, err_trunc) = err_task.await.unwrap_or_else(|_| (Vec::new(), false));
        let front_back_capture = match combined_task {
            Some(task) => task.await.ok(),
            None => None,
        };
        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        let combined = combine(&stdout, &stderr);

        Ok(ExecOutput {
            stdout,
            stderr,
            combined,
            exit_code,
            timed_out,
            cancelled,
            truncated: out_trunc || err_trunc,
            duration: started.elapsed(),
            front_back: front_back_capture,
        })
    }

    fn build_shell_command(
        &self,
        req: &ExecRequest,
        program: &str,
        path_env: Option<&str>,
    ) -> Command {
        let mut cmd = Command::new(program);
        let login = req.shell.map_or(self.login_shell, |s| s.login_arg);
        cmd.arg(if login { "-lc" } else { "-c" });
        if let Some(path) = path_env {
            cmd.env("PATH", path);
        }
        cmd.arg(&req.command);
        cmd.current_dir(&req.cwd);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &req.env {
            cmd.env(key, value);
        }
        // New process group so we can kill the whole tree, not just the shell. This is a
        // safe API — the fork-time `setpgid` lives inside std/tokio, not our crate.
        #[cfg(unix)]
        cmd.process_group(0);
        cmd
    }
}

/// Read a child stream, retaining at most `cap` bytes (drop oldest — the tail holds the
/// error/exit summary). Bounds peak memory to O(cap) rather than O(total output).
async fn read_capped<R: AsyncRead + Unpin>(reader: Option<R>, cap: usize) -> (Vec<u8>, bool) {
    let Some(mut reader) = reader else {
        return (Vec::new(), false);
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > cap {
                    let excess = buf.len() - cap;
                    buf.drain(..excess);
                    truncated = true;
                }
            }
        }
    }
    (buf, truncated)
}

/// Like [`read_capped`], additionally forwarding every chunk (arrival order)
/// to the combined collector.
async fn read_capped_forwarding<R: AsyncRead + Unpin>(
    reader: Option<R>,
    cap: usize,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) -> (Vec<u8>, bool) {
    let Some(mut reader) = reader else {
        return (Vec::new(), false);
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let _ = tx.send(chunk[..n].to_vec());
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > cap {
                    let excess = buf.len() - cap;
                    buf.drain(..excess);
                    truncated = true;
                }
            }
        }
    }
    (buf, truncated)
}

/// The combined collector: front half + back half of the char budget in
/// memory, full stream to the spill file (≤ [`SPILL_RETAIN_MAX`]), true byte
/// total. Text conversion is lossy UTF-8 with char-boundary-safe cuts.
async fn collect_front_back(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    spec: FrontBackSpec,
) -> FrontBackCapture {
    use std::collections::VecDeque;
    use tokio::io::AsyncWriteExt;

    let front_budget = spec.char_budget / 2;
    let back_budget = spec.char_budget - front_budget;
    let mut front: Vec<u8> = Vec::new();
    let mut back: VecDeque<u8> = VecDeque::new();
    let mut total: u64 = 0;
    let mut spill = None;
    let mut spill_path = None;
    if spec.spill {
        let dir = std::env::temp_dir().join("locode").join("exec");
        if tokio::fs::create_dir_all(&dir).await.is_ok() {
            static SPILL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SPILL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = dir.join(format!("cmd-{}-{}.log", std::process::id(), seq));
            if let Ok(file) = tokio::fs::File::create(&path).await {
                spill = Some(file);
                spill_path = Some(path);
            }
        }
    }
    while let Some(chunk) = rx.recv().await {
        if let Some(file) = spill.as_mut()
            && total < SPILL_RETAIN_MAX
        {
            let room = usize::try_from(SPILL_RETAIN_MAX - total).unwrap_or(usize::MAX);
            let write = &chunk[..chunk.len().min(room)];
            let _ = file.write_all(write).await;
        }
        total += chunk.len() as u64;
        if front.len() < front_budget {
            let room = front_budget - front.len();
            front.extend_from_slice(&chunk[..chunk.len().min(room)]);
            if chunk.len() > room {
                back.extend(&chunk[room..]);
            }
        } else {
            back.extend(&chunk);
        }
        while back.len() > back_budget {
            back.pop_front();
        }
    }
    if let Some(mut file) = spill {
        let _ = file.flush().await;
    }
    let truncated = total > (front.len() + back.len()) as u64;
    let front_str = String::from_utf8_lossy(&front).into_owned();
    let back_bytes: Vec<u8> = back.into_iter().collect();
    let back_str = String::from_utf8_lossy(&back_bytes).into_owned();
    if truncated {
        FrontBackCapture {
            front: front_str,
            back: Some(back_str),
            total_bytes: total,
            spill_path,
        }
    } else {
        FrontBackCapture {
            front: format!("{front_str}{back_str}"),
            back: None,
            total_bytes: total,
            spill_path,
        }
    }
}

/// Kill a running child: its whole process group on Unix (SIGTERM → grace → SIGKILL),
/// else the direct child. Best-effort; always reaps.
async fn terminate(child: &mut Child, pid: Option<u32>, grace: Duration) {
    if group_kill(child, pid, grace).await {
        return;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(unix)]
async fn group_kill(child: &mut Child, pid: Option<u32>, grace: Duration) -> bool {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    let Some(pid) = pid else { return false };
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    // Guard against a degenerate pgid (0/1) that would broadcast the signal. A child
    // spawned with `process_group(0)` leads its own group with pid > 1.
    if raw <= 1 {
        return false;
    }
    let leader = Pid::from_raw(raw);
    let _ = killpg(leader, Signal::SIGTERM);
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = killpg(leader, Signal::SIGKILL);
        let _ = child.wait().await;
    }
    true
}

#[cfg(not(unix))]
async fn group_kill(_child: &mut Child, _pid: Option<u32>, _grace: Duration) -> bool {
    false
}

/// stdout then stderr, separated by a newline when both are present.
fn combine(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n{stderr}"),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (true, true) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecLimits, Host, HostConfig, PathPolicy, test_host};
    use std::time::Duration;
    use tempfile::tempdir;

    fn req(command: &str, cwd: PathBuf) -> ExecRequest {
        ExecRequest {
            command: command.to_string(),
            cwd,
            timeout: None,
            env: Vec::new(),
            shell: None,
            front_back: None,
        }
    }

    // Tests run the shell as a NON-login shell (`-c`) so login-profile output can't
    // pollute the captured streams.
    fn shell_host(root: &std::path::Path) -> Host {
        test_host(root, PathPolicy::Jailed, false)
    }

    #[tokio::test]
    async fn captures_stdout_and_exit() {
        let dir = tempdir().unwrap();
        let host = shell_host(dir.path());
        let out = host
            .exec(
                req("echo hello", dir.path().into()),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.stdout.contains("hello"));
        assert_eq!(out.exit_code, Some(0));
        assert!(!out.timed_out && !out.cancelled);
    }

    #[tokio::test]
    async fn captures_stderr_and_nonzero_exit() {
        let dir = tempdir().unwrap();
        let host = shell_host(dir.path());
        let out = host
            .exec(
                req(">&2 echo boom; exit 3", dir.path().into()),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.stderr.contains("boom"));
        assert_eq!(out.exit_code, Some(3));
    }

    #[tokio::test]
    async fn timeout_kills_sleeper() {
        let dir = tempdir().unwrap();
        let mut config = HostConfig::new(dir.path());
        config.login_shell = false;
        config.exec = ExecLimits {
            default_timeout: Duration::from_millis(200),
            ..ExecLimits::default()
        };
        let host = Host::new(config).unwrap();
        let started = Instant::now();
        let out = host
            .exec(
                req("sleep 30", dir.path().into()),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.timed_out);
        assert!(out.exit_code.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "should not wait for the sleep"
        );
    }

    #[tokio::test]
    async fn cancellation_kills_running_command() {
        let dir = tempdir().unwrap();
        let host = shell_host(dir.path());
        let cancel = CancellationToken::new();
        let child_cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            child_cancel.cancel();
        });
        let started = Instant::now();
        let out = host
            .exec(req("sleep 30", dir.path().into()), &cancel)
            .await
            .unwrap();
        assert!(out.cancelled);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn output_over_cap_is_truncated() {
        let dir = tempdir().unwrap();
        let mut config = HostConfig::new(dir.path());
        config.login_shell = false;
        config.exec = ExecLimits {
            max_output_bytes: 1000,
            ..ExecLimits::default()
        };
        let host = Host::new(config).unwrap();
        // Print ~200 KB.
        let out = host
            .exec(
                req(
                    "for i in $(seq 1 20000); do echo 0123456789; done",
                    dir.path().into(),
                ),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.truncated);
        assert!(
            out.stdout.len() <= 1000 + 16,
            "kept ~cap bytes, got {}",
            out.stdout.len()
        );
    }

    #[tokio::test]
    async fn front_back_retains_head_and_tail_and_spills() {
        let dir = tempdir().unwrap();
        let host = shell_host(dir.path());
        let mut request = req(
            "for i in $(seq 1 2000); do echo line-$i; done",
            dir.path().into(),
        );
        request.front_back = Some(FrontBackSpec {
            char_budget: 400,
            spill: true,
        });
        let out = host.exec(request, &CancellationToken::new()).await.unwrap();
        let capture = out.front_back.expect("capture requested");
        assert!(
            capture.front.starts_with("line-1\n"),
            "front holds the head"
        );
        let back = capture.back.expect("truncated -> back present");
        assert!(
            back.ends_with("line-2000\n"),
            "back holds the tail: {back:?}"
        );
        assert!(capture.front.len() <= 200 && back.len() <= 200);
        assert!(capture.total_bytes > 400, "true total, not retained size");
        let spill = capture.spill_path.expect("spill requested");
        let spilled = std::fs::read_to_string(&spill).unwrap();
        assert!(spilled.starts_with("line-1\n") && spilled.ends_with("line-2000\n"));
        assert_eq!(spilled.len() as u64, capture.total_bytes);
        let _ = std::fs::remove_file(spill);
    }

    #[tokio::test]
    async fn front_back_small_output_is_untruncated() {
        let dir = tempdir().unwrap();
        let host = shell_host(dir.path());
        let mut request = req("echo tiny", dir.path().into());
        request.front_back = Some(FrontBackSpec {
            char_budget: 400,
            spill: false,
        });
        let out = host.exec(request, &CancellationToken::new()).await.unwrap();
        let capture = out.front_back.expect("capture requested");
        assert_eq!(capture.front, "tiny\n");
        assert!(capture.back.is_none(), "nothing cut");
        assert_eq!(capture.total_bytes, 5);
        assert!(capture.spill_path.is_none());
    }

    #[tokio::test]
    async fn shell_spec_c_arg_and_path_probe() {
        let dir = tempdir().unwrap();
        let host = shell_host(dir.path());
        let mut request = req("echo $PATH", dir.path().into());
        request.shell = Some(ShellSpec {
            detect_program: false,
            login_arg: false,
            login_path_probe: true,
        });
        let out = host.exec(request, &CancellationToken::new()).await.unwrap();
        assert_eq!(out.exit_code, Some(0));
        // The probed login PATH is non-empty and injected.
        assert!(!out.stdout.trim().is_empty());
        // Probe is cached: second call reuses the OnceCell (observable as
        // identical PATH output and no error).
        let mut request2 = req("echo $PATH", dir.path().into());
        request2.shell = Some(ShellSpec {
            detect_program: false,
            login_arg: false,
            login_path_probe: true,
        });
        let out2 = host
            .exec(request2, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.stdout, out2.stdout);
    }

    #[tokio::test]
    async fn null_stdin_does_not_hang() {
        let dir = tempdir().unwrap();
        let host = shell_host(dir.path());
        // `cat` with no args reads stdin; null stdin → immediate EOF.
        let out = host
            .exec(req("cat", dir.path().into()), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.exit_code, Some(0));
    }
}
