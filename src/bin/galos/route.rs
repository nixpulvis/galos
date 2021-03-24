use async_std::task;
use itertools::Itertools;
use structopt::StructOpt;
use indicatif::{ProgressBar, ProgressStyle};
use galos_db::{Database, systems::System};
use galos::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // #[structopt(parse(lalrpop(Route)))]
    start: String,
    end: String,
    range: f64,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&[
                    ">>>>>>",
                    "->>>>>",
                    ">->>>>",
                    ">>->>>",
                    ">>>->>",
                    ">>>>->",
                    ">>>>>-",
                    "-----",
                ])
                .template("{spinner:.yellow} {msg}"),
        );
        spinner.enable_steady_tick(250);
        spinner.set_message("Finding systems...");
        let (start, end) = task::block_on(async {
            let start = System::fetch_by_name(db, &self.start).await.unwrap();
            let end   = System::fetch_by_name(db, &self.end).await.unwrap();
            (start, end)
        });
        spinner.finish_with_message("Input systems found, finding route...");

        spinner.reset();
        let (route, cost) = start.route_to(db, &end, self.range).unwrap().unwrap();
        spinner.finish_and_clear();
        let mut last = &route[0];
        for (a, b) in route[..].into_iter().tuple_windows() {
            println!("{}", a.name);
            println!("-> {} Ly", a.distance(&b));
            last = b;
        }
        println!("{}", last.name);
        println!("total jumps ({})", cost);
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
