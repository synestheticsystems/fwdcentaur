//! `fwd admin migrate` through the CLI, against a scratch daemon of the test's own that hosts two
//! projects, alpha and beta, each with a repository of fwd's own model as the build before the move
//! onto the Commons' `git.Repository` wrote it, and alpha with a highlight in it: the migration run
//! in alpha moves alpha's repository alone, keeping its id, after which the highlight resolves
//! through it, as it does not before; beta's stays as it was; and a second run moves nothing.

mod common;

use ankurah::property::Json;
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use common::Scratch;
use fwd_core::conversion::repos::{self, before_d261};
use fwd_core::daemon::{self, Autostart, Client};
use fwd_core::model::*;
use fwd_core::project::{NamedProject, Paths};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One daemon's folders, and a checkout of each of the two projects it hosts.
struct Daemon {
    _scratch: Scratch,
    paths: Paths,
    alpha: PathBuf,
    beta: PathBuf,
}

impl Daemon {
    /// The daemon's folders in a scratch folder, and two repositories with one commit each, each
    /// the checkout of a project of its own that `tester` made.
    fn new() -> Result<Daemon> {
        let scratch = Scratch::new()?;
        let paths = scratch.paths("run", "data");
        let checkout = |name: &str| -> Result<PathBuf> {
            let root = scratch.path().join(name);
            std::fs::create_dir_all(&root)?;
            git(&root, &["init", "-q", "-b", "main"])?;
            git(&root, &["config", "user.email", "t@example.com"])?;
            git(&root, &["config", "user.name", "t"])?;
            git(&root, &["remote", "add", "origin", &upstream(name)])?;
            std::fs::write(root.join("ws.rs"), "fn open() {}\nfn retry() {}\nfn close() {}\n")?;
            git(&root, &["add", "ws.rs"])?;
            git(&root, &["commit", "-q", "-m", "init"])?;
            Ok(root)
        };
        let (alpha, beta) = (checkout("alpha")?, checkout("beta")?);
        let daemon = Daemon { paths, alpha, beta, _scratch: scratch };
        daemon.json(&daemon.alpha, &["init", "--name", "alpha", "--handle", "tester"])?;
        daemon.json(&daemon.beta, &["init", "--name", "beta", "--handle", "tester"])?;
        Ok(daemon)
    }

    /// `fwd --format json <args>` in `checkout`, reaching this daemon only, with none of the
    /// caller's settings that would choose another project, principal or scope.
    fn fwd(&self, checkout: &Path, args: &[&str]) -> Result<std::process::Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fwd"));
        command
            .current_dir(checkout)
            .args(["--format", "json"])
            .args(args)
            .env("FWD_RUNTIME_DIR", &self.paths.runtime_dir)
            .env("FWD_DATA_DIR", &self.paths.data_dir);
        for name in
            ["FWD_PROJECT", "FWD_AS", "FWD_SCOPE", "FWD_CLAUDE_SESSION", "FWD_CLAUDE_AGENT", "CLAUDE_CODE_SESSION_ID"]
        {
            command.env_remove(name);
        }
        Ok(command.output()?)
    }

    /// The JSON `fwd <args>` prints in `checkout`, which must succeed.
    fn json(&self, checkout: &Path, args: &[&str]) -> Result<Value> {
        let out = self.fwd(checkout, args)?;
        if !out.status.success() {
            return Err(anyhow!("fwd {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(serde_json::from_slice(&out.stdout)?)
    }

    /// A client of the project whose checkout is `checkout`, to write what only the build before
    /// the move wrote.
    async fn client(&self, checkout: &Path) -> Result<Client> {
        let shown = self.json(checkout, &["project", "show"])?;
        let project = NamedProject {
            id: shown["id"].as_str().ok_or_else(|| anyhow!("no id in {shown}"))?.parse()?,
            name: shown["name"].as_str().unwrap_or_default().to_string(),
        };
        daemon::ensure_daemon(&self.paths, &project, Autostart::Never).await
    }
}

fn git(root: &Path, args: &[&str]) -> Result<()> {
    fwd_core::anchoring::git(root, args)?;
    Ok(())
}

/// The upstream of the project `name`'s checkout.
fn upstream(name: &str) -> String {
    format!("git@example.com:acme/{name}.git")
}

/// Writes the project's repository as the build before the move did, named after the project and
/// registered by its checkout's upstream, and, when `highlighted`, a highlight on `ws.rs`'s second
/// line in it as the first cut of anchors stored one, made by `by`: the repository's id, and the
/// highlight's.
async fn written(client: &Client, name: &str, by: Option<EntityId>) -> Result<(EntityId, Option<EntityId>)> {
    let graph = &client.graph;
    let trx = graph.begin();
    let repo = before_d261::Repo {
        project: graph.project(),
        name: name.into(),
        remote_urls: Json::new(json!([upstream(name)])),
        default_branch: "main".into(),
    };
    let repo = trx.create(&repo).await?.id();
    let anchor = match by {
        None => None,
        Some(by) => {
            let anchor = CodeAnchor {
                project: graph.project(),
                repo,
                path: "ws.rs".into(),
                anchor_key: None,
                span_key: None,
                basis_blob: None,
                head_blob: None,
                index_blob: None,
                nearest_blob: None,
                basis_patch: None,
                blob_sha: None,
                commit_sha: None,
                side: DiffSide::Worktree,
                start_line: 1,
                end_line: 2,
                whole_file: None,
                anchored_text: "fn retry() {}\n".into(),
                context_before: "fn open() {}\n".into(),
                context_after: "fn close() {}\n".into(),
                pieces: Json::new(json!([])),
                symbol_path: None,
                created_by: by,
                created: 1_700_000_000_000,
            };
            Some(trx.create(&anchor).await?.id())
        }
    };
    trx.commit().await?;
    Ok((repo, anchor))
}

/// What a migration that finds nothing to do prints, with `moved` as its repositories moved.
fn migrated(moved: Value) -> Value {
    json!({
        "assigned": [],
        "renamed": [],
        "triage": null,
        "roles_cleared": [],
        "readdressed": [],
        "members": [],
        "visitors": [],
        "readers": [],
        "principals_moved": [],
        "scopes_cleared": [],
        "placed_in_recorded_scope": [],
        "intentions_moved": [],
        "repositories_moved": moved,
    })
}

#[tokio::test]
async fn the_migration_moves_its_projects_repositories_keeping_their_ids_and_its_highlights_resolve_through_them(
) -> Result<()> {
    let d = Daemon::new()?;
    let tester = d.json(&d.alpha, &["whoami"])?;
    let tester: EntityId = tester["principal"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("no principal in {tester}"))?
        .parse()
        .map_err(|e| anyhow!("{e:?}"))?;
    let alpha_client = d.client(&d.alpha).await?;
    let (alphas, highlight) = written(&alpha_client, "alpha", Some(tester)).await?;
    let highlight = highlight.ok_or_else(|| anyhow!("no highlight"))?.to_base64();
    let beta_client = d.client(&d.beta).await?;
    let (betas, _) = written(&beta_client, "beta", None).await?;

    let show = ["anchor", "show", &highlight];
    assert!(!d.fwd(&d.alpha, &show)?.status.success(), "its repository is found by no read until it has moved");

    let first = d.json(&d.alpha, &["admin", "migrate"])?;
    assert_eq!(first, migrated(json!([{"id": alphas.to_base64(), "name": "alpha"}])));

    let shown = d.json(&d.alpha, &show)?;
    let held = json!({
        "repo": shown["repo"],
        "path": shown["path"],
        "lines": [shown["start_line"], shown["end_line"]],
        "found": shown["resolved"]["availability"]["state"],
    });
    let expected = json!({"repo": alphas.to_base64(), "path": "ws.rs", "lines": [1, 2], "found": "present"});
    assert_eq!(held, expected, "{shown}");

    assert!(repos::any_unmoved(&beta_client.graph).await?, "beta's repository has not moved");
    let old: before_d261::RepoView = beta_client.graph.get(betas).await?;
    assert_eq!(old.name()?, "beta");

    assert_eq!(d.json(&d.alpha, &["admin", "migrate"])?, migrated(json!([])), "a second run");
    Ok(())
}
