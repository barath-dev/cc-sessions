//! Run a helper program and collect its output, with a time limit.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::os;
use crate::util::{Error, Result};

pub struct Output {
    #[allow(dead_code)] // not every system looks at it
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// The last non-empty line of stderr or stdout, for error messages.
    pub fn last_line(&self) -> String {
        let text = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_string()
    }
}

pub fn run(cmd: &mut Command, timeout: Duration) -> Result<Output> {
    os::quiet(cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error(format!("could not run {:?}: {e}", cmd.get_program())))?;

    let mut out = child.stdout.take().expect("piped stdout");
    let mut errp = child.stderr.take().expect("piped stderr");
    let out_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let err_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = errp.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error(format!("{:?} did not finish within {} seconds", cmd.get_program(), timeout.as_secs())));
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    };
    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    Ok(Output { ok: status.success(), stdout, stderr })
}
