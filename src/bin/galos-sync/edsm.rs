#![cfg(unix)]
use crate::Run;
use galos_db::Database;
use structopt::StructOpt;

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

impl Run for Cli {
    fn run(&self, _db: &Database) {
        unimplemented!();
    }
}
