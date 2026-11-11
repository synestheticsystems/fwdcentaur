//! Moves every project's store of a fwd data folder into the one store the daemon keeps for all of
//! them (docs/DECISIONS.md, D174). See README.md.

mod folder;
mod judge;
mod merge;
mod old_store;
mod records;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context as _};
use clap::Parser;
use fwd_core::daemon::CheckoutRecord;
use fwd_core::project::{Index, Paths};

use crate::folder::{Item, Staging};
use crate::merge::{Moved, Moving};
use crate::old_store::OldStore;

/// Moves every project's store of a fwd data folder into one store, in a new data folder that the
/// single-store build of fwd serves. The source is only read; the destination must not exist, and
/// appears only once the new store is judged against the old ones.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The data folder whose projects each have a store of their own (stop its daemon first).
    #[arg(long)]
    from: PathBuf,
    /// Where to write the data folder with the one store.
    #[arg(long)]
    to: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            println!("refused, nothing was written to {}: {e:#}", args.to.display());
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &Args) -> anyhow::Result<()> {
    let items = folder::survey(&args.from)?;
    let index = Index::load(&Paths { runtime_dir: PathBuf::new(), data_dir: args.from.clone() })?;
    let mut moving = Vec::new();
    for item in &items {
        if let Item::Store { project, path } = item {
            let Some(entry) = index.projects.iter().find(|entry| entry.id == *project) else {
                bail!("{} is the store of a project the index does not list", path.display());
            };
            let old = OldStore::read(&args.from.join(path)).with_context(|| format!("project {project}"))?;
            moving.push(Moving { uuid: *project, name: entry.name.clone(), old });
        }
    }

    let staging = Staging::create(&args.from, &args.to)?;
    println!("moving the stores of {} into {}", args.from.display(), args.to.display());
    for item in &items {
        match item {
            Item::Dir(relative) => staging.add_dir(&args.from, relative)?,
            Item::File(relative) => staging.add_file(&args.from, relative)?,
            Item::Store { .. } | Item::Checkouts { .. } | Item::Cache(_) => {}
        }
    }
    let store = staging.path.join("store.sqlite");
    let moved = merge::write(&store, &moving).await?;
    for item in &items {
        if let Item::Checkouts { project, path } = item {
            let renumbered = moving.iter().zip(&moved).find(|(m, _)| m.uuid == *project).map(|(_, done)| done);
            rebind(&args.from.join(path), &staging.path.join(path), renumbered, project)?;
        }
    }
    let old: Vec<(uuid::Uuid, PathBuf)> = items
        .iter()
        .filter_map(|item| match item {
            Item::Store { project, path } => Some((*project, args.from.join(path))),
            _ => None,
        })
        .collect();
    let old: Vec<(uuid::Uuid, &Path)> = old.iter().map(|(p, path)| (*p, path.as_path())).collect();
    judge::judge(&store, &old).map_err(|e| anyhow::anyhow!("the new store does not match the old ones: {e}"))?;

    for (project, done) in moving.iter().zip(&moved) {
        let made =
            done.made.iter().filter(|(_, n)| *n > 0).map(|(model, n)| format!("{n} {model}")).collect::<Vec<_>>();
        println!(
            "{}: {} records ({}), {} fields set in a second pass, {} ids naming nothing carried as they were",
            project.uuid,
            done.renumbered.len(),
            made.join(", "),
            done.patched,
            done.dangling.len()
        );
        let mut dangling = std::collections::BTreeMap::new();
        for d in &done.dangling {
            *dangling.entry(format!("{}.{}", d.model, d.field)).or_insert(0) += 1;
        }
        for (field, count) in dangling {
            println!("  ids naming nothing, carried as they were: {count} in {field}");
        }
        for (kind, count) in project.old.migrate_would_change() {
            println!("  carried as they are, though `fwd admin migrate` would still change them: {count} {kind}");
        }
    }
    let to = staging.settle(&args.from)?;
    println!("moved into {}", to.display());
    Ok(())
}

/// Carries a project's checkout registry with each scope binding naming the scope's new id; a
/// binding to a scope the project does not hold is carried as it was, and said.
fn rebind(from: &Path, to: &Path, moved: Option<&Moved>, project: &uuid::Uuid) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(from).with_context(|| format!("reading {}", from.display()))?;
    let mut checkouts: Vec<CheckoutRecord> =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", from.display()))?;
    for checkout in &mut checkouts {
        if let Some(scope) = &checkout.scope {
            match moved.and_then(|done| done.renumbered.get(scope)) {
                Some(new) => checkout.scope = Some(new.to_base64()),
                None => println!("{project}: a checkout is bound to scope {scope}, which the project does not hold; carried as it was"),
            }
        }
    }
    std::fs::write(to, serde_json::to_vec_pretty(&checkouts)?).with_context(|| format!("writing {}", to.display()))?;
    Ok(())
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod tests;
