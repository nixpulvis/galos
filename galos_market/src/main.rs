//! What the galaxy trades
//!
//! Reads the galos database, and writes nothing to it.
use eframe::egui;
use galos_market::market::Database;
use galos_market::{Markets, Queries, quiet_scrolled_tables};
use std::time::Duration;

/// How long the window waits before looking for an answer again
///
/// Egui draws when something happens to it and then sleeps, and an answer
/// arriving on a channel is not something that happens to it. So while a
/// question is out, the window is asked to wake up and look.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Listen to what egui and everything else has to say
///
/// Egui reports what it thinks is wrong through `log` and nowhere else: a
/// widget taking another's id, a font it has no size to match, a pane closed
/// that nothing could close. None of it is heard unless a logger is
/// installed, and there is no sign that anything was said.
///
/// Warnings and errors by default, since that is what wants reading. `RUST_LOG`
/// overrides it, so `RUST_LOG=galos_market=debug,egui=debug` is there when a
/// warning turns out not to be enough.
fn listen() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn"),
    )
    .init();
}

fn main() -> eframe::Result {
    listen();

    let db = match async_std::task::block_on(Database::new()) {
        Ok(db) => db,
        Err(err) => {
            eprintln!("no database: {}", err);
            eprintln!("set DATABASE_URL, or put one in .env");
            std::process::exit(1);
        }
    };

    let mut markets = Markets::default();
    let mut queries = Queries::new(db);
    queries.ask(Markets::opening());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Galos - Markets")
            .with_inner_size([1200., 800.]),
        ..Default::default()
    };

    eframe::run_ui_native("galos_market", options, move |ui, _frame| {
        quiet_scrolled_tables(ui.ctx());

        for answer in queries.answers() {
            markets.take(answer);
        }

        for ask in markets.draw(ui) {
            queries.ask(ask);
        }

        if queries.outstanding() > 0 {
            ui.ctx().request_repaint_after(LOOK_AGAIN);
        }
    })
}
