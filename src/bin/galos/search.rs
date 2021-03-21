use async_std::task;
use structopt::StructOpt;
use galos_db::{Database, systems::System};
use galos::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    #[structopt(name = "QUERY")]
    query: String,

    #[structopt(short = "d", long = "cube")]
    diameter: Option<f32>,

    #[structopt(short = "r", long = "sphere")]
    radius: Option<f32>,

    // #[structopt(short = "f", long = "filter", parse(from_filter_string))]
    // filters: Vec<String>,

    // TODO: What is the best way to handle filters for systems, factions, etc.
    // We don't want full SQL obviously.
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        dbg!(self);

        task::block_on(async {
            let system = System::fetch_by_name(db, &self.query).await.unwrap();
            dbg!(system);
        });
    }
}
