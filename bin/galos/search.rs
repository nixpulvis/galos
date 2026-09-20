use async_std::task;
use clap::Args;
use galos_db::{bodies::Body, factions::Faction, systems::System, Database};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

#[allow(dead_code)]
#[derive(Args, Debug)]
pub struct Cli {
    /// Systems:
    ///     *Sol
    ///     *LHS%
    /// Factions:
    ///     @newp
    ///     @New LHS 3728 Alliance
    /// Systems + Factions:
    ///     *Sol@    Stars named Sol and their factions
    ///     *@newp   Stars with newp factions (not null)

    /// One of these two is required and they are mutually exclusive,
    /// which clap says for itself: the pair used to fall through to a
    /// hand-printed help page, and printing usage is what a parser is
    /// for.
    #[arg(
        short = 's',
        long = "systems",
        value_name = "SYSTEM(s)",
        required_unless_present = "faction_like",
        conflicts_with = "faction_like"
    )]
    pub system_like: Option<String>,

    #[arg(short = 'f', long = "factions", value_name = "FACTION(s)")]
    pub faction_like: Option<String>,

    #[arg(short = 'd', long)]
    pub diameter: Option<f64>,

    #[arg(short = 'r', long)]
    pub radius: Option<f64>,

    #[arg(short = 'c', long)]
    pub count: bool,

    /// The index directory the names are searched in.
    ///
    /// The database cannot answer a fragment any more, and it is not a
    /// shortcoming of the SQL: a name its address spells is not stored
    /// there at all, so `ILIKE` would answer for the exceptions and call
    /// it the galaxy. The published names table holds every name, stored
    /// or spelled, and searches it in microseconds.
    #[arg(short = 'i', long = "index", default_value = ".galos_index")]
    pub index: String,
    // #[structopt(short = "f", long = "filter", parse(from_filter_string))]
    // pub filters: Vec<String>,

    // TODO: What is the best way to handle filters for systems, factions, etc.
    // We don't want full SQL obviously.
}

impl Cli {
    /// Answer the search, printing what it found.
    pub fn run(&self, db: &Database) {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&[
                    ">>><<<", ">>--<<", ">----<", "------", ">----<", ">>--<<",
                    ">>><<<",
                ])
                .template("{spinner:.yellow} {msg}")
                .unwrap(),
        );
        spinner.enable_steady_tick(Duration::from_millis(125));

        task::block_on(async {
            match (self.system_like.as_ref(), self.faction_like.as_ref()) {
                (Some(query), None) => {
                    let found = match matched(&self.index, query) {
                        Ok(found) => found,
                        Err(said) => {
                            spinner.finish_and_clear();
                            eprintln!("{said}");
                            return;
                        }
                    };
                    // The index says which systems are meant and the
                    // database says everything else about them: one read a
                    // hit, by address, which is a primary-key lookup.
                    let mut systems = Vec::with_capacity(found.len());
                    for address in found {
                        match System::fetch(db, address).await {
                            Ok(system) => systems.push(system),
                            // A system the index names and the database has
                            // never held is not an error here: the two are
                            // built from different reads and either may be
                            // ahead.
                            Err(_) => continue,
                        }
                    }
                    // A radius asks for the sky around what matched, which
                    // is the database's question rather than the index's.
                    if let Some(radius) = self.radius {
                        systems = systems
                            .iter()
                            .flat_map(|system| system.neighbors(db, radius))
                            .collect();
                    }

                    spinner.finish_and_clear();

                    if self.count {
                        println!("{} systems found.", systems.len());
                    } else {
                        for system in systems {
                            print_system(&system);

                            let bodies = Body::fetch_all(db, system.address)
                                .await
                                .unwrap();
                            if !bodies.is_empty() {
                                println!("\tbodies:");
                                for body in bodies {
                                    println!("\t\t- {}", body.name);
                                }
                            }
                        }
                    }
                }

                (None, Some(query)) => {
                    let factions =
                        Faction::fetch_like_name(db, &query).await.unwrap();

                    spinner.finish_and_clear();

                    if self.count {
                        println!("{} factions found.", factions.len());
                    } else {
                        for faction in factions {
                            println!("{:?}", faction)
                        }
                    }
                }

                // Refused as the command line is parsed: exactly one of
                // the two is required, and `conflicts_with` forbids the
                // pair. See the fields above.
                (Some(_), Some(_)) | (None, None) => unreachable!(
                    "clap requires exactly one of --systems and --factions"
                ),
            }
        });
    }
}

/// Which systems the query names, off the published names table.
///
/// The whole search, and it is not the database's: the table holds every
/// name whether it was stored or is spelled from the address, answers a
/// prefix in microseconds and a word held anywhere in a name — `A*` for
/// `SAGITTARIUS A*` — in a few milliseconds. See
/// `galos_index::names::Table::matching`.
///
/// A percent sign is what the old SQL pattern wanted and this does not, so
/// it is trimmed rather than searched for: nobody typing `LHS%` means a
/// system with a percent in its name.
fn matched(dir: &str, query: &str) -> Result<Vec<i64>, String> {
    let names = galos_index::Names::open(std::path::Path::new(dir))
        .map_err(|err| format!("reading the names table at {dir}: {err}"))?;
    if names.is_empty() {
        return Err(format!(
            "{dir} publishes no names; build one with `galos index build \
             --from database --dir {dir}`"
        ));
    }
    let query = galos_index::SystemName::new(query.trim_matches('%'));
    Ok(names
        .matching(&query, RESULTS)
        .into_iter()
        .map(|entry| entry.address)
        .collect())
}

/// How many systems a search answers with.
///
/// The old SQL answered with however many matched, which for `%sol%` over a
/// galaxy is a screenful nobody reads and a scan nobody wants.
const RESULTS: usize = 50;

fn print_system(system: &System) {
    print!("{}: ", system.name);
    if let Some(position) = system.position {
        print!("({}, {}, {})", position.x, position.y, position.z);
    }
    println!("");
    if system.population > 0 {
        println!("\tpopulation: {}", system.population);
    }
    if let Some(security) = system.security {
        println!("\tsecurity: {:?}", security);
    }
    if let Some(government) = system.government {
        println!("\tgovernment: {:?}", government);
    }
    if let Some(allegiance) = system.allegiance {
        println!("\tallegiance: {:?}", allegiance);
    }
    if let Some(economies) = system.economies {
        print!("\teconomy: {:?}", economies.primary);
        if let Some(secondary) = economies.secondary {
            print!("/{:?}", secondary);
        }
        println!("");
    }
}
