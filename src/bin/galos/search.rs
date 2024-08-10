#[cfg(unix)]
use async_std::task;
#[cfg(unix)]
use galos::Run;
#[cfg(unix)]
use galos_db::{
    factions::{Faction, SystemFaction},
    systems::System,
    Database,
};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;
use structopt::StructOpt;

#[allow(dead_code)]
#[derive(StructOpt, Debug)]
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

    #[structopt(short = "s", long = "systems", name = "SYSTEM(s)")]
    pub system_like: Option<String>,

    #[structopt(short = "f", long = "factions", name = "FACTION(s)")]
    pub faction_like: Option<String>,

    #[structopt(short = "d", long = "diameter")]
    pub diameter: Option<f64>,

    #[structopt(short = "r", long = "radius")]
    pub radius: Option<f64>,

    #[structopt(short = "c", long = "count")]
    pub count: bool,
    // #[structopt(short = "f", long = "filter", parse(from_filter_string))]
    // pub filters: Vec<String>,

    // TODO: What is the best way to handle filters for systems, factions, etc.
    // We don't want full SQL obviously.
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&[
                    ">>><<<", ">>--<<", ">----<", "------", ">----<", ">>--<<", ">>><<<",
                ])
                .template("{spinner:.yellow} {msg}")
                .unwrap(),
        );
        spinner.enable_steady_tick(Duration::from_millis(125));

        task::block_on(async {
            match (self.system_like.as_ref(), self.faction_like.as_ref()) {
                (Some(query), faction_like) => {
                    let systems = if let Some(radius) = self.radius {
                        System::fetch_in_range_like_name(db, radius, &query)
                            .await
                            .unwrap()
                    } else {
                        System::fetch_like_name(db, &query).await.unwrap()
                    };

                    spinner.finish_and_clear();

                    if self.count {
                        println!("{} systems found.", systems.len());
                    } else {
                        for system in systems {
                            print_system(&system);

                            if let Some(faction_query) = faction_like {
                                if faction_query == "%" {
                                    let sfs = SystemFaction::fetch_all(db, Some(system.address))
                                        .await
                                        .unwrap();
                                    for (name, sf) in sfs {
                                        println!("\t{}", name.to_lowercase());
                                        println!("\t\tinfluence: {}%", sf.influence * 100.);
                                        println!("\t\tstate: {:?}", sf.state);
                                        println!("\t\thappiness: {:?}", sf.happiness);
                                        println!("\t\tupdated_at: {}", sf.updated_at);
                                    }
                                }
                            }
                        }
                    }
                }

                (None, Some(ref query)) => {
                    let factions = Faction::fetch_like_name(db, query).await.unwrap();

                    spinner.finish_and_clear();

                    if self.count {
                        println!("{} factions found.", factions.len());
                    } else {
                        for faction in factions {
                            println!("{:?}", faction)
                        }
                    }
                }

                (None, None) => {
                    // XXX: Why is -r being printed after the next shell prompt?!
                    Cli::clap().print_help().expect("issue printing help")
                }
            }
        });
    }
}

fn print_system(system: &System) {
    print!("{}: ", system.name);
    if let Some(position) = system.position {
        print!("({}, {}, {})", position.x, position.y, position.z);
    }
    println!("\t\t[{}]", system.updated_at);
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
    if let Some(primary_economy) = system.primary_economy {
        print!("\teconomy: {:?}", primary_economy);
        if let Some(secondary_economy) = system.secondary_economy {
            print!("/{:?}", secondary_economy);
        }
        println!("");
    }
}
