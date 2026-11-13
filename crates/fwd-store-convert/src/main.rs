//! Converts a fwd data folder whose project stores Ankurah f8e85d2 wrote into a new data folder
//! that Ankurah 5d8a000b, fwd's pin, reads. See README.md.

mod f8e85d2_to_5d8a000b;
mod folder;
mod store;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::f8e85d2_to_5d8a000b::{FROM_REVISION, TO_REVISION};
use crate::folder::{Item, Staging};

/// Converts a fwd data folder written at Ankurah f8e85d2 into a new one that Ankurah 5d8a000b reads.
/// The source is only read; the destination must not exist, and appears only once every project's
/// store in it is converted and read back.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The data folder to convert (stop its daemon first).
    #[arg(long)]
    from: PathBuf,
    /// Where to write the converted data folder.
    #[arg(long)]
    to: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match convert(&args).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Converts the folder; `Ok(false)` when a store was refused, after saying which and why.
async fn convert(args: &Args) -> anyhow::Result<bool> {
    let items = folder::survey(&args.from)?;
    let staging = Staging::create(&args.from, &args.to)?;
    println!(
        "converting {} (Ankurah {}) into {} (Ankurah {})",
        args.from.display(),
        &FROM_REVISION[..8],
        args.to.display(),
        &TO_REVISION[..8]
    );
    let mut refused = 0;
    let copied = async {
        for item in &items {
            match item {
                Item::Dir(relative) => staging.add_dir(&args.from, relative)?,
                Item::File(relative) => staging.add_file(&args.from, relative)?,
                Item::Store { project, path } => {
                    match store::convert(&args.from.join(path), &staging.path.join(path)).await {
                        Ok(report) => println!(
                            "{project}: {} entities, {} events, {} event ids changed",
                            report.entities, report.events, report.ids_changed
                        ),
                        Err(e) => {
                            refused += 1;
                            println!("{project}: refused: {e}");
                        }
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = copied {
        drop(staging);
        return Err(e.context(format!("nothing was written to {}", args.to.display())));
    }
    if refused > 0 {
        drop(staging);
        println!("{refused} store(s) refused; nothing was written to {}", args.to.display());
        return Ok(false);
    }
    let to = staging.settle(&args.from)?;
    println!("converted into {}", to.display());
    Ok(true)
}

#[cfg(test)]
mod tests;
