//! Starting the real binary with admin authentication required and reading its
//! console, for the tests of the admin plane's protection.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::{free_port, write_config};

pub const TOKEN_MARKER: &str = "admin token (shown once, store it now): ";

/// A proxy started with authentication required and its console output.
pub struct Running {
    pub child: Child,
    pub admin: std::net::SocketAddr,
    lines: mpsc::Receiver<String>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn start(dir: &Path, env: &[(&str, &str)], extra_args: &[&str]) -> Result<Running, String> {
    start_with_config(dir, "", env, extra_args)
}

/// Like [`start`], adding `extra_config` lines to the `[data_plane]` section.
pub fn start_with_config(
    dir: &Path,
    extra_config: &str,
    env: &[(&str, &str)],
    extra_args: &[&str],
) -> Result<Running, String> {
    let data_port = free_port();
    let admin_port = free_port();
    let config = write_config(dir, data_port, admin_port, extra_config);
    let mut command = Command::new(env!("CARGO_BIN_EXE_fast-proxy"));
    command
        .arg("--config")
        .arg(&config)
        .args(["--admin-auth", "required"])
        .args(extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let admin: std::net::SocketAddr = ([127, 0, 0, 1], admin_port).into();
    let mut running = Running {
        child,
        admin,
        lines,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = running.child.try_wait().unwrap() {
            return Err(format!("exited at startup ({status})"));
        }
        if std::net::TcpStream::connect(admin).is_ok() {
            return Ok(running);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("did not start listening".into())
}

pub fn start_ok(dir: &Path, env: &[(&str, &str)]) -> Running {
    start_ok_with_args(dir, env, &[])
}

/// Like [`start_ok`], passing command-line flags too. A "free" port can be
/// taken before the proxy binds it, so a failed start is retried on new ports.
pub fn start_ok_with_args(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Running {
    start_ok_with_config(dir, "", env, args)
}

/// Like [`start_ok_with_args`], adding `extra_config` lines to `[data_plane]`.
pub fn start_ok_with_config(
    dir: &Path,
    extra_config: &str,
    env: &[(&str, &str)],
    args: &[&str],
) -> Running {
    for _ in 0..5 {
        if let Ok(running) = start_with_config(dir, extra_config, env, args) {
            return running;
        }
    }
    panic!("the proxy would not start");
}

/// The console lines printed so far.
pub fn printed(running: &Running) -> Vec<String> {
    thread::sleep(Duration::from_millis(200));
    running.lines.try_iter().collect()
}

pub fn token_in(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .find_map(|line| line.strip_prefix(TOKEN_MARKER))
        .map(str::to_string)
}
