use structopt::StructOpt;
use galos_db::Database;
use crate::SyncDb;

#[derive(StructOpt, Debug)]
pub enum Cli {
    File(FileCli),
    Api(ApiCli),
}

#[derive(StructOpt, Debug)]
pub struct FileCli {
    // TODO: Type as a path.
    // TODO: Default, when not provided?
    #[structopt(name = "PATH")]
    pub path: String,
}

// TODO: -s single system, -S systems query?
#[derive(StructOpt, Debug)]
pub struct ApiCli {}

impl SyncDb for Cli {
    fn sync_db(&self, db: &Database) {
        unimplemented!();
    }
}

