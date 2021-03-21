use async_std::task;
use structopt::StructOpt;
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
        dbg!(self);

        task::block_on(async {
            if let Some(radius) = self.radius {
                let systems = System::fetch_in_range_by_name(db, radius, &self.query).await.unwrap();
                if self.count {
                    dbg!(systems.len());
                } else {
                    dbg!(systems);
                }
            } else {
                let system = System::fetch_by_name(db, &self.query).await.unwrap();
                dbg!(system);
            }
        });
    }
}
