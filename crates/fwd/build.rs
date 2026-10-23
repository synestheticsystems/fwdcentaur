//! The web UI (crates/fwd-web, built by trunk into crates/fwd-web/dist) is embedded into the
//! binary by serve.rs with `include_dir!`, which needs the directory to exist at compile time
//! even when nobody has built the UI yet. This creates it, and asks cargo to rebuild `fwd`
//! whenever its contents change (a fresh `scripts/build-web.sh` run). It also names the build
//! (`FWD_BUILD`), which `fwd --version` prints, `fwd capabilities` gives and the daemon gives on
//! `/health`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let manifest = Path::new(&manifest);
    let dist = manifest.join("../fwd-web/dist");
    std::fs::create_dir_all(&dist).expect("creating crates/fwd-web/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rustc-env=FWD_BUILD={}", build(manifest));
}

/// The build's name, so that an agent or a person can tell which build answers them
/// (`fwd --version`, `fwd capabilities`), and a page the daemon served can tell when the daemon
/// restarted on another build and offer to load the new page (docs/design/web-ui.md).
/// `FWD_BUILD` from the environment when it is set, for a build that has to carry a name of its
/// own (a release, or a check that needs two builds with different names); otherwise, as the VS
/// Code extension's version.mjs names its builds, the crate's version, the number of commits
/// behind the checkout's commit, `.g` and its short hash, and `.dirty.<UTC time>` when the
/// checkout has changes not committed (`0.0.1-<commits>.g<short hash>[.dirty.20260929120000]`);
/// outside a git checkout, the crate's version alone. Cargo runs this again when the page, the sources of
/// `fwd` and `fwd-core`, the lock file, the checkout's commit or `FWD_BUILD` change, so a build
/// from a checkout with changes gets a new time whenever what goes into the binary changed.
fn build(manifest: &Path) -> String {
    println!("cargo:rerun-if-env-changed=FWD_BUILD");
    if let Some(given) = std::env::var("FWD_BUILD").ok().filter(|given| !given.is_empty()) {
        return given;
    }
    for input in ["src", "Cargo.toml", "../fwd-core/src", "../fwd-core/Cargo.toml", "../../Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", manifest.join(input).display());
    }
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    // --no-optional-locks: asking must not rewrite the index, which a build may run beside other
    // git work.
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("--no-optional-locks")
            .args(args)
            .current_dir(manifest)
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let (Some(commits), Some(hash), Some(changes)) = (
        git(&["rev-list", "--count", "HEAD"]),
        git(&["rev-parse", "--short", "HEAD"]),
        git(&["status", "--porcelain", "--untracked-files=no"]),
    ) else {
        return version;
    };
    // Git adds a line to HEAD's log whenever the checkout's commit changes (a commit, a checkout,
    // a merge, a reset), in a worktree as in the main checkout.
    if let Some(log) = git(&["rev-parse", "--git-path", "logs/HEAD"]).map(|log| manifest.join(log)) {
        if log.exists() {
            println!("cargo:rerun-if-changed={}", log.display());
        }
    }
    let dirty = if changes.is_empty() { String::new() } else { format!(".dirty.{}", utc_now()) };
    format!("{version}-{commits}.g{hash}{dirty}")
}

/// Now in UTC as `YYYYMMDDHHMMSS`, the date from Howard Hinnant's `civil_from_days`.
fn utc_now() -> String {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, time) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}{:02}{:02}{:02}", time / 3600, time % 3600 / 60, time % 60)
}
