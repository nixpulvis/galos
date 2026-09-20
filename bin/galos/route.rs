use async_std::task;
use clap::Args;
use galos_db::{
    systems::{nav::ModuleClass, System},
    Database,
};
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use prettytable::{format, Table};
use std::time::Duration;

#[allow(dead_code)]
#[derive(Args, Debug)]
pub struct Cli {
    // #[structopt(parse(lalrpop(Route)))]
    pub start: String,
    pub end: String,

    #[arg(default_value_t = 7.5, short = 'r', long)]
    pub range: f64,
    #[arg(default_value_t = 25.0, short = 'm', long)]
    pub total_mass: f64,
    #[arg(default_value_t = 48.0, short = 'o', long)]
    pub optimized_mass: f64,

    #[arg(default_value_t = 2, short = 's', long)]
    pub size: u8,
    #[arg(default_value = "E", short = 'c', long)]
    pub class: ModuleClass,
}

impl Cli {
    /// Plot the route, printing what it found.
    pub fn run(&self, db: &Database) {
        let spinner = ProgressBar::new_spinner();
        spinner.enable_steady_tick(Duration::from_millis(100));
        spinner.set_message("Finding systems...");
        let (start, end) = task::block_on(async {
            let start = System::fetch_by_name(db, &self.start).await.unwrap();
            let end = System::fetch_by_name(db, &self.end).await.unwrap();
            (start, end)
        });
        spinner.finish_with_message("Input systems found, finding route...");

        spinner.reset();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&[
                    ">>>>>>>>>>>>>>>>",
                    "->>>>>>>>>>>>>>>",
                    ">->>>>>>>>>>>>>>",
                    ">>->>>>>>>>>>>>>",
                    ">>>->>>>>>>>>>>>",
                    ">>>>->>>>>>>>>>>",
                    ">>>>>->>>>>>>>>>",
                    ">>>>>>->>>>>>>>>",
                    ">>>>>>>->>>>>>>>",
                    ">>>>>>>>->>>>>>>",
                    ">>>>>>>>>->>>>>>",
                    ">>>>>>>>>>->>>>>",
                    ">>>>>>>>>>>->>>>",
                    ">>>>>>>>>>>>->>>",
                    ">>>>>>>>>>>>>->>",
                    ">>>>>>>>>>>>>>->",
                    ">>>>>>>>>>>>>>>-",
                    "----------------",
                ])
                .template("{spinner:.yellow} {msg}")
                .unwrap(),
        );
        spinner.enable_steady_tick(Duration::from_millis(250));

        let mut table = Table::new();
        table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
        table.set_titles(row!["Origin", "Destination", "Distance"]);
        let (route, cost) = start.route_to(db, &end, self.range).unwrap();
        spinner.finish_and_clear();
        let mut gross = 0.;
        for (a, b) in route[..].into_iter().tuple_windows() {
            let d = a.distance(&b);
            table.add_row(row![a.name, b.name, format!("{:.2} Ly", d)]);
            gross += d;
        }
        table.printstd();
        println!(
            "jumps: {:.2}, path: {:.2} Ly, distance: {:.2} Ly",
            cost,
            gross,
            route[0].distance(&route.last().expect("valid route"))
        );
    }
}

// enum Route {
//     End,
//     Stop(String),
//     // `A -> B` specifies a direct path from A to B
//     Path(Box<Route>, Box<Route>),
//     // `A + B` specifies a path to both A and B, where the route could either visit
//     // A or B first
//     Both(Box<Route>, Box<Route>),
//     // `A | B` specifies a path to either A or B
//     Either(Box<Route>, Box<Route>),
// }
