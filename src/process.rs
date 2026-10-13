//! Running child processes with a timeout, behind a trait so tests can fake it.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub enum RunError {
    Spawn(std::io::Error),
    Timeout,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Spawn(e) => write!(f, "could not start: {e}"),
            RunError::Timeout => write!(f, "timed out"),
        }
    }
}

pub trait CommandRunner: Send + Sync {
    /// Run `program`; when `path` is given it becomes the child's `PATH`.
    fn run_in(
        &self,
        program: &Path,
        args: &[String],
        timeout: Duration,
        path: Option<&OsStr>,
    ) -> Result<Output, RunError>;

    fn run(&self, program: &Path, args: &[String], timeout: Duration) -> Result<Output, RunError> {
        self.run_in(program, args, timeout, None)
    }
}

/// Runs every command with a fixed `PATH`. A Finder-launched app has a minimal
/// `PATH`, which breaks `#!/usr/bin/env node` scripts such as npm's `claude`.
pub struct WithPath<'a> {
    pub inner: &'a dyn CommandRunner,
    pub path: OsString,
}

impl CommandRunner for WithPath<'_> {
    fn run_in(
        &self,
        program: &Path,
        args: &[String],
        timeout: Duration,
        _path: Option<&OsStr>,
    ) -> Result<Output, RunError> {
        self.inner.run_in(program, args, timeout, Some(&self.path))
    }
}

/// Runs real processes. stdin is closed; on Windows no console window is shown.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run_in(
        &self,
        program: &Path,
        args: &[String],
        timeout: Duration,
        path: Option<&OsStr>,
    ) -> Result<Output, RunError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(path) = path {
            cmd.env("PATH", path);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let started = Instant::now();
        let mut child = cmd.spawn().map_err(RunError::Spawn)?;
        // Drain both pipes on threads so a chatty child can't block on a full pipe.
        let stdout = Drain::start(child.stdout.take());
        let stderr = Drain::start(child.stderr.take());
        let Some(status) = child.wait_timeout(timeout).map_err(RunError::Spawn)? else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout);
        };
        // A background process the child started (e.g. from a shell rc file) can
        // hold the pipes open after the child exits, so only wait for the
        // drains until the deadline (plus a short grace), then take what arrived.
        let deadline = (started + timeout).max(Instant::now() + DRAIN_GRACE);
        while !(stdout.finished() && stderr.finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(Output {
            success: status.success(),
            code: status.code(),
            stdout: stdout.text(),
            stderr: stderr.text(),
        })
    }
}

const DRAIN_GRACE: Duration = Duration::from_millis(200);

/// Reads a pipe on a thread into a shared buffer, so the text read so far is
/// available even if the pipe never closes.
struct Drain {
    buf: Arc<Mutex<Vec<u8>>>,
    handle: JoinHandle<()>,
}

impl Drain {
    fn start<R: Read + Send + 'static>(pipe: Option<R>) -> Drain {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&buf);
        let handle = std::thread::spawn(move || {
            let Some(mut pipe) = pipe else { return };
            let mut chunk = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });
        Drain { buf, handle }
    }

    fn finished(&self) -> bool {
        self.handle.is_finished()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf.lock().unwrap()).into_owned()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, timeout_ms: u64) -> Result<Output, RunError> {
        SystemRunner.run(
            Path::new("/bin/sh"),
            &["-c".to_string(), script.to_string()],
            Duration::from_millis(timeout_ms),
        )
    }

    #[test]
    fn captures_stdout_stderr_and_exit_code() {
        let out = sh("echo hello; echo oops >&2; exit 3", 5_000).unwrap();
        assert_eq!(out.stdout, "hello\n");
        assert_eq!(out.stderr, "oops\n");
        assert_eq!(out.code, Some(3));
        assert!(!out.success);
    }

    #[test]
    fn times_out_and_kills_the_child() {
        assert!(matches!(sh("sleep 5", 200), Err(RunError::Timeout)));
    }

    /// A background process started by the child (e.g. from a shell rc file)
    /// inherits the output pipes and keeps them open after the child exits.
    #[test]
    fn background_grandchild_cannot_hold_the_run_past_its_timeout() {
        let started = std::time::Instant::now();
        let out = sh("sleep 4 & echo hi", 300).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(out.stdout, "hi\n");
    }

    #[test]
    fn path_override_reaches_the_child() {
        let script = vec!["-c".to_string(), "printf %s \"$PATH\"".to_string()];
        let path = std::ffi::OsStr::new("/custom/bin:/usr/bin:/bin");
        let out = SystemRunner
            .run_in(
                Path::new("/bin/sh"),
                &script,
                Duration::from_secs(5),
                Some(path),
            )
            .unwrap();
        assert_eq!(out.stdout, "/custom/bin:/usr/bin:/bin");
        let wrapped = WithPath {
            inner: &SystemRunner,
            path: path.to_os_string(),
        };
        let out = wrapped
            .run(Path::new("/bin/sh"), &script, Duration::from_secs(5))
            .unwrap();
        assert_eq!(out.stdout, "/custom/bin:/usr/bin:/bin");
    }

    #[test]
    fn missing_program_is_a_spawn_error() {
        let err = SystemRunner
            .run(
                Path::new("/definitely/not/here"),
                &[],
                Duration::from_secs(1),
            )
            .unwrap_err();
        assert!(matches!(err, RunError::Spawn(_)));
    }
}
