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
    /// The data plane's HTTP listener.
    pub data: std::net::SocketAddr,
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
    let (sender, lines) = mpsc::channel();
    let (child, admin, data) = attempt(dir, (extra_config, env, extra_args), sender)?;
    Ok(Running {
        child,
        admin,
        data,
        lines,
    })
}

/// The child and its admin and data addresses, once it listens.
type Started = (Child, std::net::SocketAddr, std::net::SocketAddr);

/// Starts one proxy on fresh ports, sending its console lines to `console`. A
/// child that does not come up is killed.
fn attempt(
    dir: &Path,
    (extra_config, env, extra_args): (&str, &[(&str, &str)], &[&str]),
    console: mpsc::Sender<String>,
) -> Result<Started, String> {
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
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if console.send(line).is_err() {
                break;
            }
        }
    });
    let admin: std::net::SocketAddr = ([127, 0, 0, 1], admin_port).into();
    let data: std::net::SocketAddr = ([127, 0, 0, 1], data_port).into();
    let outcome = wait_listening(&mut child, admin);
    if outcome.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    outcome.map(|()| (child, admin, data))
}

/// Waits until `admin` answers and the child is still running a moment later:
/// a parallel test can briefly hold the port picked for this proxy, so a
/// connection may reach that listener while this proxy fails to bind and exits.
fn wait_listening(child: &mut Child, admin: std::net::SocketAddr) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return Err(format!("exited at startup ({status})"));
        }
        if std::net::TcpStream::connect(admin).is_ok() {
            thread::sleep(Duration::from_millis(300));
            return match child.try_wait().unwrap() {
                Some(status) => Err(format!(
                    "exited at startup ({status}); the port was someone else's"
                )),
                None => Ok(()),
            };
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
/// The console lines of failed attempts are kept: one that lost its port may
/// already have created the database and printed the bootstrap token, which
/// the retry will not print again.
pub fn start_ok_with_config(
    dir: &Path,
    extra_config: &str,
    env: &[(&str, &str)],
    args: &[&str],
) -> Running {
    let (sender, lines) = mpsc::channel();
    let mut last_error = String::new();
    for _ in 0..5 {
        match attempt(dir, (extra_config, env, args), sender.clone()) {
            Ok((child, admin, data)) => {
                return Running {
                    child,
                    admin,
                    data,
                    lines,
                }
            }
            Err(error) => last_error = error,
        }
    }
    panic!("the proxy would not start: {last_error}");
}

/// The console lines printed so far.
pub fn printed(running: &Running) -> Vec<String> {
    thread::sleep(Duration::from_millis(200));
    running.lines.try_iter().collect()
}

/// The bootstrap token a freshly started proxy prints, waiting for the line to
/// arrive (a slow machine can take longer than any fixed pause); `None` if none
/// is printed within ten seconds.
pub fn bootstrap_token(running: &Running) -> Option<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match running.lines.recv_timeout(left) {
            Ok(line) => {
                if let Some(token) = line.strip_prefix(TOKEN_MARKER) {
                    return Some(token.to_string());
                }
            }
            Err(_) => return None,
        }
    }
    None
}

pub fn token_in(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .find_map(|line| line.strip_prefix(TOKEN_MARKER))
        .map(str::to_string)
}
