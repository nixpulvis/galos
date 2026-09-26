//! `galos route`: a route between two systems.
//!
//! Over an index directory with `--index`, which is the router the map
//! plots with ([`galos_route`]) and needs no database; over the database
//! otherwise, where the build has one.

use clap::Args;
use galos_index::{FsSource, Names, Sky, Source, SystemName};
use galos_route::{Boosts, Drive, Jumps, Routing, Tuning};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[allow(dead_code)]
#[derive(Args, Debug)]
pub struct Cli {
    // #[structopt(parse(lalrpop(Route)))]
    pub start: String,
    pub end: String,

    #[arg(default_value_t = 7.5, short = 'r', long)]
    pub range: f64,

    /// Route over this index directory rather than the database.
    ///
    /// The map's own router, over the places the cell payloads carry and
    /// the supercharge table beside them. Required in a build with no
    /// database.
    #[arg(short = 'i', long, value_name = "DIR")]
    pub index: Option<PathBuf>,

    #[cfg(feature = "db")]
    #[arg(default_value_t = 25.0, short = 'm', long)]
    pub total_mass: f64,
    #[cfg(feature = "db")]
    #[arg(default_value_t = 48.0, short = 'o', long)]
    pub optimized_mass: f64,

    #[cfg(feature = "db")]
    #[arg(default_value_t = 2, short = 's', long)]
    pub size: u8,
    #[cfg(feature = "db")]
    #[arg(default_value = "E", short = 'c', long)]
    pub class: galos_db::systems::nav::ModuleClass,
}

impl Cli {
    /// Plot the route, printing what it found.
    pub async fn run(&self) -> Result<(), String> {
        match &self.index {
            Some(dir) => self.over_index(dir).await,
            #[cfg(feature = "db")]
            None => {
                let db = galos_db::Database::new()
                    .await
                    .map_err(|err| format!("no database to ask: {err}"))?;
                self.over_database(&db);
                Ok(())
            }
            #[cfg(not(feature = "db"))]
            None => Err("no database in this build: name an index \
                         directory with --index"
                .into()),
        }
    }

    /// The route over an index directory, with the router the map uses.
    async fn over_index(&self, dir: &Path) -> Result<(), String> {
        let said = |what: &'static str| {
            let dir = dir.display().to_string();
            move |err: std::io::Error| format!("{dir}: {what}: {err}")
        };
        let names = Names::open(dir).map_err(said("the names table"))?;
        let sky = Arc::new(Sky::open(dir).map_err(said("the cells"))?);
        let boosts = FsSource::new(dir)
            .boosts()
            .await
            .map_err(said("the supercharge table"))?
            .map_or_else(Boosts::absent, Boosts::of);

        let end = |name: &str| -> Result<(i64, [f64; 3]), String> {
            let address = names
                .address_of(SystemName::new(name).as_str())
                .ok_or_else(|| format!("no system named {name}"))?;
            let place = sky
                .placed(address)
                .ok_or_else(|| format!("{name} is named but not placed"))?;
            Ok((address, place))
        };
        let (start, end) = (end(&self.start)?, end(&self.end)?);

        let graph = Jumps::over(Arc::clone(&sky))
            .built(&boosts)
            .ok_or("the index has no galaxy to route over")?;
        let hops = graph
            .route(
                start,
                end,
                self.range,
                Routing::default(),
                Drive::default(),
                Tuning::default(),
                None,
            )
            .ok_or_else(|| {
                format!(
                    "no route from {} to {} at {} ly",
                    self.start, self.end, self.range
                )
            })?;

        let name = |address: i64| {
            names
                .name_of(address)
                .map_or_else(|| address.to_string(), Into::into)
        };
        let apart = |a: [f64; 3], b: [f64; 3]| {
            a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt()
        };
        let mut gross = 0.;
        for pair in hops.windows(2) {
            let ((a, at), (b, to)) = (pair[0], pair[1]);
            let d = apart(at, to);
            gross += d;
            println!("{:<32} {:<32} {d:>8.2} Ly", name(a), name(b));
        }
        println!(
            "jumps: {}, path: {gross:.2} Ly, distance: {:.2} Ly",
            hops.len().saturating_sub(1),
            apart(start.1, end.1)
        );
        Ok(())
    }

    /// The route over the database.
    #[cfg(feature = "db")]
    fn over_database(&self, db: &galos_db::Database) {
        use async_std::task;
        use galos_db::systems::System;
        use indicatif::{ProgressBar, ProgressStyle};
        use itertools::Itertools;
        use prettytable::{format, Table};
        use std::time::Duration;

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
