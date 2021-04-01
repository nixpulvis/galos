use async_std::task;
use itertools::Itertools;
use structopt::StructOpt;
use indicatif::{ProgressBar, ProgressStyle};
use prettytable::{format, Table};
use galos_db::{Database, systems::{ModuleClass, System}};
use galos::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // #[structopt(parse(lalrpop(Route)))]
    start: String,
    end: String,

    #[structopt(default_value = "7.5", short = "r", long)]
    range: f64,
    #[structopt(default_value = "25", short = "m", long)]
    total_mass: f64,
    #[structopt(default_value = "48", short = "o", long)]
    optimized_mass: f64,

    #[structopt(default_value = "2", short = "s", long)]
    size: u8,
    #[structopt(default_value = "E", short = "c", long)]
    class: ModuleClass,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let spinner = ProgressBar::new_spinner();
        spinner.enable_steady_tick(100);
        spinner.set_message("Finding systems...");
        let (start, end) = task::block_on(async {
            let start = System::fetch_by_name(db, &self.start).await.unwrap();
            let end   = System::fetch_by_name(db, &self.end).await.unwrap();
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
                .template("{spinner:.yellow} {msg}"),
        );
        spinner.enable_steady_tick(250);

        let mut table = Table::new();
        table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
        table.set_titles(row!["Origin", "Destination", "Distance"]);
        let (route, cost) = start.route_to(db, &end, self.range).unwrap().unwrap();
        spinner.finish_and_clear();
        let mut gross = 0.;
        for (a, b) in route[..].into_iter().tuple_windows() {
            let d = a.distance(&b);
            table.add_row(row![a.name, b.name, format!("{:.2} Ly", d)]);
            gross += d;
        }
        table.printstd();
        println!("jumps: {:.2}, path: {:.2} Ly, distance: {:.2} Ly",
            cost,
            gross,
            route[0].distance(&route.last().expect("valid route")));
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
