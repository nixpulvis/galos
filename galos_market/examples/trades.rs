//! Put the three trade questions to the real database, and time them
//!
//! ```sh
//! cargo run --example trades         # around Sol, and the busiest market in reach
//! cargo run --example trades -- LAVE  # or somewhere you name
//! ```
use galos_market::market::{Database, Error};
use galos_market::trade::{Comparison, Filters, Trade, between};
use std::time::Instant;

#[async_std::main]
async fn main() -> Result<(), Error> {
    let db = Database::new().await?;
    let filters = Filters::default();
    let origin = std::env::args().nth(1).unwrap_or("SOL".into());
    println!("{:?}\n", filters);

    // Somewhere to stand, which is the question a trader actually has.
    let started = Instant::now();
    let near = Trade::near(&db, &origin, &filters).await?;
    println!(
        "near {} within {:?} ly: {} rows in {:?}",
        origin,
        filters.within,
        near.len(),
        started.elapsed()
    );
    for trade in near.iter().take(5) {
        say(trade, filters.hold);
    }
    println!();

    let started = Instant::now();
    let board = Trade::anywhere(&db, &filters).await?;
    println!("anywhere: {} rows in {:?}", board.len(), started.elapsed());
    for trade in board.iter().take(5) {
        say(trade, filters.hold);
    }

    let Some(anchor) = board.first().map(|t| t.source.clone()) else {
        println!("\nnothing to anchor on");
        return Ok(());
    };

    println!(
        "\nfrom {} in {} ({}), within {:?} ly",
        anchor.station_name,
        anchor.system_name,
        anchor.market_id,
        filters.within
    );
    let started = Instant::now();
    let from = Trade::from_market(&db, anchor.market_id, &filters).await?;
    println!("from_market: {} rows in {:?}", from.len(), started.elapsed());
    for trade in from.iter().take(5) {
        say(trade, filters.hold);
    }

    // Widening it to the galaxy is the same question with the bound taken
    // off, and is what says whether the bound is doing any work.
    let wide = Filters { within: None, ..filters.clone() };
    let started = Instant::now();
    let far = Trade::from_market(&db, anchor.market_id, &wide).await?;
    println!("\nunbounded: {} rows in {:?}", far.len(), started.elapsed());
    for trade in far.iter().take(3) {
        say(trade, filters.hold);
    }

    let Some(other) = from.first().map(|t| t.destination.clone()) else {
        println!("\nnowhere within reach to compare against");
        return Ok(());
    };

    let started = Instant::now();
    let both =
        Comparison::fetch_all(&db, anchor.market_id, other.market_id).await?;
    let apart = between(&db, anchor.market_id, other.market_id).await?;
    println!(
        "\n{} against {}, {:?} ly apart: {} commodities in common, in {:?}",
        anchor.station_name,
        other.station_name,
        apart.map(|ly| ly.round()),
        both.len(),
        started.elapsed()
    );

    let mut worth = both.clone();
    worth.sort_by(|a, b| b.out().cmp(&a.out()));
    for row in worth.iter().take(5) {
        println!(
            "  {:<28} out {:>8}  back {:>8}",
            row.name,
            row.out().map(|n| n.to_string()).unwrap_or("-".into()),
            row.back().map(|n| n.to_string()).unwrap_or("-".into()),
        );
    }

    Ok(())
}

fn say(trade: &Trade, hold: i32) {
    println!(
        "  {:<26} {:>7} -> {:>8} = {:>7}/t  {:>6}t  {:>10} total  {:>7} ly  \
         {} -> {}",
        trade.name,
        trade.buy_price,
        trade.sell_price,
        trade.margin(),
        trade.tons(hold),
        trade.haul(hold),
        trade.distance.map(|ly| format!("{:.0}", ly)).unwrap_or("?".into()),
        trade.source.station_name,
        trade.destination.station_name,
    );
}
