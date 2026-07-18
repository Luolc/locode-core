//! Shell execution: capture stdout/stderr/exit, hard timeout, output cap, group-kill.

use std::path::PathBuf;
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
        let started = Instant::now();
        let timeout = req
            .timeout
            .unwrap_or(self.limits.default_timeout)
            .min(self.limits.max_timeout);
        let cap = self.limits.max_output_bytes;

        let mut child = self
            .build_command(&req)
            .spawn()
            .map_err(|e| ExecError::Spawn(e.to_string()))?;
        let pid = child.id();

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out_task = tokio::spawn(read_capped(stdout, cap));
        let err_task = tokio::spawn(read_capped(stderr, cap));

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
        })
    }

    fn build_command(&self, req: &ExecRequest) -> Command {
        let mut cmd = Command::new(&self.shell_program);
        cmd.arg(if self.login_shell { "-lc" } else { "-c" });
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
