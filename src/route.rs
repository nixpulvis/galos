use structopt::StructOpt;
use galos_db::Database;
use crate::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // #[structopt(parse(lalrpop(Route)))]
    route: String,
}

impl Run for Cli {
    fn run(&self, _db: &Database) {
        unimplemented!();
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
