//! Saying what a journal directory holds.
//!
//! ```sh
//! cargo run -p galos_journal -- info ~/Saved\ Games/Frontier\ Developments/Elite\ Dangerous
//! ```
//!
//! One subcommand, and it writes nothing. Reading a journal *into* something
//! — a database, or an index directory kept current while the game runs — is
//! `galos-sync`, which is where every other publisher is read from and where
//! the flags for choosing between the two live:
//!
//! ```sh
//! galos-sync journal ~/Saved\ Games/…/Elite\ Dangerous --watch
//! galos-sync journal ~/Saved\ Games/…/Elite\ Dangerous --to index=.galos_journal_index --watch
//! ```
//!
//! What is left here is the thing that belongs to this crate rather than to
//! that program: point it at a directory and it says what the accumulator
//! made of it, without a database, an index directory or a byte written
//! anywhere. It is how you find out whether a journal directory is the one
//! you meant, and it is this crate's own smoke test against a real one.

use clap::{Parser, Subcommand};
use galos_journal::JournalSource;
use std::path::{Path, PathBuf};

/// Read an Elite: Dangerous journal directory and say what is in it.
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
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "galos_journal=info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Info { journal } => say(&read(&journal)),
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
