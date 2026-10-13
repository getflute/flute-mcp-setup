//! Helpers shared by unit tests.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::process::{CommandRunner, Output, RunError};

type Respond = dyn Fn(&Path, &[String]) -> Result<Output, RunError> + Send + Sync;

/// A `CommandRunner` that records every call and answers from a closure.
pub struct FakeRunner {
    calls: Mutex<Vec<Vec<String>>>,
    paths: Mutex<Vec<Option<String>>>,
    respond: Box<Respond>,
}

impl FakeRunner {
    pub fn new(
        respond: impl Fn(&Path, &[String]) -> Result<Output, RunError> + Send + Sync + 'static,
    ) -> FakeRunner {
        FakeRunner {
            calls: Mutex::new(Vec::new()),
            paths: Mutex::new(Vec::new()),
            respond: Box::new(respond),
        }
    }

    /// Every call so far as `[program, args...]`.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    /// The `PATH` each call was given, in call order.
    pub fn paths(&self) -> Vec<Option<String>> {
        self.paths.lock().unwrap().clone()
    }
}

impl CommandRunner for FakeRunner {
    fn run_in(
        &self,
        program: &Path,
        args: &[String],
        _timeout: Duration,
        path: Option<&OsStr>,
    ) -> Result<Output, RunError> {
        let mut call = vec![program.display().to_string()];
        call.extend(args.iter().cloned());
        self.calls.lock().unwrap().push(call);
        let path = path.map(|p| p.to_string_lossy().into_owned());
        self.paths.lock().unwrap().push(path);
        (self.respond)(program, args)
    }
}

pub fn ok(stdout: &str) -> Output {
    Output {
        success: true,
        code: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
    }
}

pub fn fail(code: i32, stderr: &str) -> Output {
    Output {
        success: false,
        code: Some(code),
        stdout: String::new(),
        stderr: stderr.to_string(),
    }
}

/// `name` as an executable file name on this host (`name.exe` on Windows).
pub fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Create an executable file (and its parent dirs) and return its path.
pub fn touch_exe(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_path_buf()
}
