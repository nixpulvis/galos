//! Reading a commander's journal directory into an index.
//!
//! ```sh
//! # What is in it
//! cargo run -p galos_journal -- info ~/Saved\ Games/Frontier\ Developments/Elite\ Dangerous
//!
//! # Write it out as an index directory the map can be pointed at
//! cargo run -p galos_journal -- build ~/Saved\ Games/.../Elite\ Dangerous .galos_journal_index
//!
//! # And keep it current while the game is running
//! cargo run -p galos_journal -- watch ~/Saved\ Games/.../Elite\ Dangerous .galos_journal_index
//! ```
//!
//! Database-free, as everything reading a journal into the index vocabulary
//! is. `build` and `watch` write the same layout `galos-db index` writes, so
//! the result is readable by `galos-index info` and by the map — though the
//! map does not need either of them, holding a
//! [`JournalSource`](galos_journal::JournalSource) directly and layering it
//! over the published index instead. These exist to look at what a journal
//! says on its own, and to keep a directory current for something that can
//! only read a directory.

use clap::{Parser, Subcommand};
use galos_journal::JournalSource;
use galos_journal::source::EVERY;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;

/// Read an Elite: Dangerous journal directory as a galaxy index.
#[derive(Parser)]
#[command(name = "galos-journal", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Say what a journal directory holds, without writing anything.
    Info {
        /// The journal directory the game writes its `.log` files to.
        journal: PathBuf,
    },
    /// Read a journal directory once and write an index directory from it.
    Build {
        /// The journal directory the game writes its `.log` files to.
        journal: PathBuf,
        /// Where to write the index.
        #[arg(default_value = ".galos_journal_index")]
        dir: PathBuf,
    },
    /// Follow a journal directory, republishing the index as the game writes.
    Watch {
        /// The journal directory the game writes its `.log` files to.
        journal: PathBuf,
        /// Where to write the index.
        #[arg(default_value = ".galos_journal_index")]
        dir: PathBuf,
        /// Seconds between passes over the directory.
        #[arg(long, default_value_t = EVERY.as_secs())]
        every: u64,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "galos_journal=info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Info { journal } => {
            let source = read(&journal);
            say(&source);
        }
        Command::Build { journal, dir } => {
            let source = read(&journal);
            say(&source);
            publish(&source, &dir);
        }
        Command::Watch { journal, dir, every } => {
            let source = read(&journal);
            say(&source);
            publish(&source, &dir);
            watch(&source, &dir, Duration::from_secs(every.max(1)));
        }
    }
}

/// Read the whole directory once, ending the process where it could not be.
///
/// A journal directory that is not there is the commonest thing to get wrong
/// about running any of this, and the path tried is the useful half of saying
/// so.
fn read(journal: &Path) -> JournalSource {
    let source = JournalSource::new(journal);
    if let Err(err) = source.pass() {
        eprintln!("cannot read the journal at {}: {err}", journal.display());
        std::process::exit(1);
    }
    source
}

/// What the reading holds, one line, in the order a reader wants it.
fn say(source: &JournalSource) {
    let counts = source.counts();
    let of = |what: &str| counts.get(what).copied().unwrap_or(0);
    println!(
        "{}: commander {}, {} systems ({} drawn in {} cells), \
{} named, {} reaching, {} supercharging, {} populated",
        source.dir().display(),
        source.commander(),
        of("systems"),
        of("drawn"),
        of("cells"),
        of("names"),
        of("reaches"),
        of("boosts"),
        of("populated"),
    );
}

/// Write the index directory, ending the process where it could not be.
fn publish(source: &JournalSource, dir: &Path) {
    if let Err(err) = source.publish(dir) {
        eprintln!("cannot write the index at {}: {err}", dir.display());
        std::process::exit(1);
    }
    info!(dir = %dir.display(), "published");
}

/// Follow the journal, republishing whenever it says something new.
///
/// Republished on change rather than on the beat: the directory is written
/// whole, so a pass that read nothing would otherwise rewrite the lot every
/// second for the good of nobody. Never returns; stopped with a signal, as
/// `galos-db index --watch` is.
fn watch(source: &JournalSource, dir: &Path, every: Duration) {
    loop {
        std::thread::sleep(every);
        match source.pass() {
            Ok(pass) if pass.rebuilt => {
                publish(source, dir);
                say(source);
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("the journal could not be read: {err}");
            }
        }
    }
}
