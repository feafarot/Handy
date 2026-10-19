//! Meeting notes written by the Claude Code CLI, signed in with the user's
//! Claude account: each request runs `claude -p` once, with the instructions
//! as the system prompt and the text on stdin, no tools, no MCP servers and
//! no settings, from the temp directory so no project's CLAUDE.md is read.
//!
//! `--bare` would skip more, but it never reads the Claude account login.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

/// Longer than any notes request should take; a hung CLI is killed after it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

/// Claude models take far more than any meeting; this only bounds how much
/// transcript goes in one request.
pub const CONTEXT_TOKENS: u32 = 200_000;

pub struct ClaudeCode {
    program: PathBuf,
    model: String,
}

/// The JSON `claude -p --output-format json` prints.
#[derive(Deserialize)]
struct Output {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: String,
}

impl ClaudeCode {
    pub fn new(model: &str) -> Result<Self> {
        let program = find_program().ok_or_else(|| {
            anyhow!("Claude Code is not installed (the `claude` command was not found)")
        })?;
        let model = model.trim();
        Ok(Self {
            program,
            model: if model.is_empty() { "haiku" } else { model }.to_string(),
        })
    }

    pub fn ask(&self, instructions: &str, text: String) -> Result<String> {
        let mut command = Command::new(&self.program);
        command.args([
            "-p",
            "--model",
            &self.model,
            "--tools",
            "",
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--no-session-persistence",
            "--output-format",
            "json",
            "--system-prompt",
            instructions,
        ]);
        let (status, stdout, stderr) = run(command, Some(text), REQUEST_TIMEOUT)?;
        match serde_json::from_str::<Output>(&stdout) {
            Ok(output) if output.is_error => Err(anyhow!("Claude Code: {}", output.result)),
            Ok(output) if status => Ok(output.result),
            _ => Err(anyhow!(
                "Claude Code failed: {}",
                first_line(&stderr)
                    .or(first_line(&stdout))
                    .unwrap_or("no output")
            )),
        }
    }
}

/// The installed Claude Code's version, e.g. "2.1.302 (Claude Code)".
pub fn version() -> Result<String> {
    let program = find_program().ok_or_else(|| anyhow!("the `claude` command was not found"))?;
    let mut command = Command::new(program);
    command.arg("--version");
    let (status, stdout, stderr) = run(command, None, VERSION_TIMEOUT)?;
    if !status {
        return Err(anyhow!(
            "`claude --version` failed: {}",
            first_line(&stderr).unwrap_or("no output")
        ));
    }
    first_line(&stdout)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("`claude --version` printed nothing"))
}

fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// `claude` on PATH, else where its installer puts it: apps started from the
/// desktop often get a shorter PATH than a terminal.
fn find_program() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let installed = [
        home.as_ref().map(|home| home.join(".local").join("bin")),
        home.as_ref().map(|home| home.join(".claude").join("local")),
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/usr/local/bin")),
    ];
    on_path
        .into_iter()
        .chain(installed.into_iter().flatten())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// Runs `command` with `input` on stdin; kills it after `timeout`. Returns
/// whether it succeeded, and its stdout and stderr.
fn run(
    mut command: Command,
    input: Option<String>,
    timeout: Duration,
) -> Result<(bool, String, String)> {
    command
        .current_dir(std::env::temp_dir())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().context("could not start Claude Code")?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // On a thread, so a full pipe can't block us while the CLI writes.
        std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let stdout = read_on_thread(child.stdout.take());
    let stderr = read_on_thread(child.stderr.take());
    let success = wait(&mut child, timeout)?;
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    Ok((success, stdout, stderr))
}

fn read_on_thread(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    })
}

fn wait(child: &mut Child, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "Claude Code did not answer within {} minutes",
                timeout.as_secs() / 60
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the real CLI with the user's account; run by hand with
    /// `cargo test claude -- --ignored`.
    #[test]
    #[ignore]
    fn asks_the_installed_claude_code() {
        assert!(version().unwrap().contains("Claude Code"));
        let claude = ClaudeCode::new("haiku").unwrap();
        let reply = claude
            .ask(
                "Reply with the names in the text, comma-separated, nothing else.",
                "[0:01] Me: Hi Dana.\n[0:02] Speaker 1: Hi, I'm Dana.".into(),
            )
            .unwrap();
        assert!(reply.contains("Dana"), "{reply}");
        let error = ClaudeCode::new("no-such-model")
            .unwrap()
            .ask("Say hi.", "hi".into())
            .unwrap_err();
        assert!(error.to_string().starts_with("Claude Code"), "{error}");
    }
}
