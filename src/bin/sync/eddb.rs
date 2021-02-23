use structopt::StructOpt;
use galos_db::Database;
use crate::SyncDb;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // TODO: Type as a path.
    // TODO: Default, when not provided?
    #[structopt(name = "PATH")]
    pub path: String,
}

impl SyncDb for Cli {
    fn sync_db(&self, db: &Database) {
        unimplemented!();
    }
}
