//! Drive the whole thing without a window
//!
//! Asks the real database the three questions the panes ask, folds each
//! answer in, and paints the result into a bare egui context, printing what
//! came out. Everything the window does except open one:
//!
//! ```sh
//! cargo run --example smoke -- gold | less
//! ```
use eframe::egui;
use galos_market::market::{Database, Error};
use galos_market::{Markets, Queries};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How long to wait on the database before giving up on it
const PATIENCE: Duration = Duration::from_secs(30);

#[async_std::main]
async fn main() -> Result<(), Error> {
    let db = Database::new().await?;
    let name = std::env::args().nth(1).unwrap_or("gold".into());

    let mut markets = Markets::default();
    let mut queries = Queries::new(db);

    queries.ask(Markets::opening());
    wait(&mut markets, &mut queries);
    println!("{} commodities", markets.commodities.len());

    let ask = markets.pick(&name);
    queries.ask(ask);
    wait(&mut markets, &mut queries);
    println!("{} markets carry {}", markets.quotes.len(), name);

    // What the table would draw first, which is the best price paid for it
    // anywhere.
    let Some(&first) = markets.shown().first() else {
        println!("nowhere to open");
        return Ok(());
    };
    let quote = markets.quotes[first].clone();
    let ask = markets.open(&quote);
    queries.ask(ask);
    wait(&mut markets, &mut queries);
    println!(
        "{} in {} trades {} commodities",
        quote.station_name,
        quote.system_name,
        markets.board.len()
    );

    println!("\nwhat the window would say, in the order it says it:");
    for word in painted(&mut markets) {
        println!("  {}", word);
    }

    Ok(())
}

/// Wait for every question out to be answered, folding each answer in
fn wait(markets: &mut Markets, queries: &mut Queries) {
    let started = Instant::now();
    while queries.outstanding() > 0 && started.elapsed() < PATIENCE {
        for answer in queries.answers() {
            markets.take(answer);
        }
        sleep(Duration::from_millis(10));
    }

    if let Some(trouble) = &markets.trouble {
        panic!("{}", trouble);
    }
}

/// Every piece of text the three panes paint, in the order they paint it
fn painted(markets: &mut Markets) -> Vec<String> {
    let ctx = egui::Context::default();
    let output = ctx.run_ui(egui::RawInput::default(), |ui| {
        markets.draw(ui);
    });

    fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().into()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    text_of(shape, into);
                }
            }
            _ => {}
        }
    }

    let mut said = Vec::new();
    for shape in &output.shapes {
        text_of(&shape.shape, &mut said);
    }
    said
}
