//! `galos`, both ways round
//!
//! With a command, ask it and print the answer. Without one, hand the
//! terminal to the UI, which asks the same commands and draws the same
//! answers. Everything either of them does lives in the [`galos`] crate; this
//! is only the fork in the road.

use clap::Parser;
use galos::cli::{self, Format};
use galos::query::Query;
use galos::tui;
use galos_db::Database;

#[derive(Parser, Debug)]
#[command(
    name = "galos",
    version,
    about = "Query the galaxy",
    long_about = "Ask about systems, factions, stations, bodies and routes.\n\
                  With no command, opens the terminal UI, where the same \
                  commands can be typed with `:`."
)]
struct Cli {
    /// Override the default (.env) database URL
    #[arg(short = 'd', long = "database", value_name = "URL")]
    database_url: Option<String>,

    /// How to print the answer
    #[arg(long, value_enum, default_value = "table")]
    format: Format,

    #[command(subcommand)]
    query: Option<Query>,
}

#[async_std::main]
async fn main() {
    // Said rather than returned. `Result` from `main` reports what went wrong
    // with `Debug`, and `NoRoute { start: "Sol", end: "Meliae", range: 40.0 }`
    // is the shape of our error type where the user asked about a route.
    if let Err(err) = galos(Cli::parse()).await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn galos(cli: Cli) -> Result<(), galos::Error> {
    let db = match &cli.database_url {
        Some(url) => Database::from_url(url).await?,
        None => Database::new().await?,
    };

    match cli.query {
        Some(query) => cli::run(&query, &db, cli.format).await?,
        // The UI runs the terminal itself and blocks until the user is done
        // with it, which is not something to do on the executor's thread:
        // every query it asks is spawned there.
        None => async_std::task::spawn_blocking(move || tui::run(db))
            .await
            .map_err(|err| {
                galos::Error::Nonsense(format!("terminal: {err}"))
            })?,
    }

    Ok(())
}
