use async_std::task;
use structopt::StructOpt;
use indicatif::{ProgressBar, ProgressStyle};
use galos_db::{Database, systems::System};
use galos::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    #[structopt(name = "QUERY")]
    query: String,

    #[structopt(short = "d", long = "diameter")]
    diameter: Option<f64>,

    #[structopt(short = "r", long = "radius")]
    radius: Option<f64>,

    #[structopt(short = "c", long = "count")]
    count: bool,

    // #[structopt(short = "f", long = "filter", parse(from_filter_string))]
    // filters: Vec<String>,

    // TODO: What is the best way to handle filters for systems, factions, etc.
    // We don't want full SQL obviously.
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&[
                    ">>><<<",
                    ">>--<<",
                    ">----<",
                    "------",
                    ">----<",
                    ">>--<<",
                    ">>><<<",
                ])
                .template("{spinner:.yellow} {msg}"),
        );
        spinner.enable_steady_tick(125);

        task::block_on(async {
            let systems = if let Some(radius) = self.radius {
                System::fetch_in_range_like_name(db, radius, &self.query).await.unwrap()
            } else {
                System::fetch_like_name(db, &self.query).await.unwrap()
            };

            spinner.finish_and_clear();

            if self.count {
                println!("{} systems found.", systems.len());
            } else {
                for system in systems { print_system(&system) }
            }
        });
    }
}

fn print_system(system: &System) {
    println!("{}: ({}, {}, {})",
        system.name,
        system.position.x, system.position.y, system.position.z);
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
