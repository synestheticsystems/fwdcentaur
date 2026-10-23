//! Which build answers (task 2IiSsfv8, "Report the build, with its git suffix, from fwd --version
//! and fwd capabilities as /health does"): `fwd --version` prints the build crates/fwd/build.rs
//! names, `fwd capabilities` gives it as `build`, and the daemon the binary starts gives the same
//! on `/health`, so that an agent or a person can tell one build from another by asking fwd.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// A scratch folder holding the runtime and data folders of the daemon the test starts, which is
/// stopped when the test ends, whether it passed or failed.
struct Scratch(tempfile::TempDir);

impl Scratch {
    /// What `fwd <args>` prints, reaching this scratch daemon only, which a command starts from
    /// this binary (with no `FWD_DAEMON_BIN` naming another).
    fn fwd(&self, args: &[&str]) -> Result<String> {
        let dir = self.0.path();
        let out = Command::new(env!("CARGO_BIN_EXE_fwd"))
            .current_dir(dir)
            .env("FWD_RUNTIME_DIR", dir.join("run"))
            .env("FWD_DATA_DIR", dir.join("data"))
            .env("FWD_DEV_RUNNER_DIR", dir.join("dev-runner"))
            .env_remove("FWD_DAEMON_BIN")
            .args(args)
            .output()?;
        if !out.status.success() {
            bail!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
        }
        Ok(String::from_utf8(out.stdout)?)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.fwd(&["daemon", "stop"]);
    }
}

/// The daemon's answer on `/health`, asked over loopback with the `Host` the daemon answers to, as
/// fwd-core's client asks it.
fn health(port: u64) -> Result<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", u16::try_from(port)?))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(stream, "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer)?;
    let (head, body) = answer.split_once("\r\n\r\n").ok_or_else(|| anyhow!("no body in {answer:?}"))?;
    if !head.starts_with("HTTP/1.1 200") {
        bail!("the health check was refused: {head}");
    }
    Ok(serde_json::from_str(body)?)
}

/// `fwd --version` prints `fwd` and the build the daemon gives on `/health`, and `fwd capabilities`
/// gives the same as `build`; built in a git checkout, as this test is, with no `FWD_BUILD` given,
/// that build names the checkout's commit after the crate's version (`0.0.1-646.g184fd78`), where
/// both commands gave the crate's version alone until 2026-09-30.
#[test]
fn version_and_capabilities_name_the_build_health_names() -> Result<()> {
    let scratch = Scratch(tempfile::tempdir()?);
    let started: Value = serde_json::from_str(&scratch.fwd(&["daemon", "start"])?)?;
    let port = started["port"].as_u64().ok_or_else(|| anyhow!("no port in {started}"))?;
    let health = health(port)?;
    let build = health["build"].as_str().ok_or_else(|| anyhow!("no build in {health}"))?;

    assert_eq!(scratch.fwd(&["--version"])?, format!("fwd {build}\n"));
    let capabilities: Value = serde_json::from_str(&scratch.fwd(&["capabilities", "--format", "json"])?)?;
    assert_eq!(capabilities["build"], build);

    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .ok()
        .filter(|out| out.status.success());
    if let (Some(head), None) = (head, std::env::var_os("FWD_BUILD")) {
        let head = String::from_utf8(head.stdout)?;
        let suffix = build
            .strip_prefix(concat!(env!("CARGO_PKG_VERSION"), "-"))
            .ok_or_else(|| anyhow!("the build {build} does not start with the crate's version and a hyphen"))?;
        let mut parts = suffix.split('.');
        let (commits, hash) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        assert!(commits.parse::<u64>().is_ok(), "the build {build} names no number of commits");
        let hash = hash.strip_prefix('g').filter(|hash| !hash.is_empty());
        assert!(
            hash.is_some_and(|hash| head.starts_with(hash)),
            "the build {build} does not name the checkout's commit {}",
            head.trim()
        );
    }
    Ok(())
}
