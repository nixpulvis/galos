use async_std::task;
use structopt::StructOpt;
use galos_db::{Database, systems::{System, Node}};
use galos::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // #[structopt(parse(lalrpop(Route)))]
    range: f64,
    start: String,
    end: String,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let (start, end) = task::block_on(async {
            let start: Node = System::fetch_by_name(db, &self.start).await.unwrap().into();
            let end: Node   = System::fetch_by_name(db, &self.end).await.unwrap().into();
            (start, end)
        });

        let (route, cost) = start.route_to(db, &end, self.range).unwrap().unwrap();
        println!("total cost {}", cost);
        let mut a = &start;
        for b in &route {
            if a != b {
                let d = a.distance(&b);
                println!("{} -{}> {}", a.address, d, b.address);
                a = b;
            }
        }
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
