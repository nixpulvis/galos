//! Asking the database without stopping the drawing
//!
//! A question goes out as an [`Ask`] and comes back as an [`Answer`], with a
//! thread doing the waiting in between. Nothing in the drawing holds a
//! connection or an await point, which is what lets the panes be moved into
//! the map: `bevy` spawns its tasks its own way, and only this module would
//! be written again.
//!
//! Every answer names what was asked. A commodity is picked, and picked
//! again before the first answer arrives, and both answers land in whatever
//! order they finish in. Neither is wrong about what it was asked, so the
//! reader is left to drop the one it no longer wants.
use crate::market::{Commodity, Database, Quote, Summary};
use crate::trade::{Comparison, Filters, Trade, between};
use async_std::task;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

/// A question for the database
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// Every commodity traded anywhere, and what the galaxy makes of it
    Commodities,
    /// Everywhere one commodity trades, by name
    Quotes(String),
    /// Everything one market trades, by market id
    Board(i64),
    /// What is worth carrying, from the given market or from anywhere
    Trades(Option<i64>, Filters),
    /// The best run to be had around a named system, both ends near it
    Near(String, Filters),
    /// What two markets would pay each other, and how far apart they are
    Compare(i64, i64),
}

/// What came back
#[derive(Debug)]
pub enum Answer {
    Commodities(Vec<Summary>),
    Quotes(String, Vec<Quote>),
    Board(i64, Vec<Commodity>),
    Trades(Option<i64>, Vec<Trade>),
    Near(String, Vec<Trade>),
    Compare(i64, i64, Vec<Comparison>, Option<f64>),
    /// A question the database would not answer, and what it said instead
    Trouble(Ask, String),
}

/// The questions outstanding, and the way answers come home
pub struct Queries {
    db: Database,
    /// Handed to each task to answer down
    home: Sender<Answer>,
    answers: Receiver<Answer>,
    outstanding: usize,
}

impl Queries {
    pub fn new(db: Database) -> Self {
        let (home, answers) = channel();
        Queries { db, home, answers, outstanding: 0 }
    }

    /// Put a question, to be answered whenever it is answered
    pub fn ask(&mut self, ask: Ask) {
        let db = self.db.clone();
        let home = self.home.clone();
        self.outstanding += 1;

        task::spawn(async move {
            let answer = match &ask {
                Ask::Commodities => {
                    Summary::fetch_all(&db).await.map(Answer::Commodities)
                }
                Ask::Quotes(name) => Quote::fetch_all(&db, name)
                    .await
                    .map(|quotes| Answer::Quotes(name.clone(), quotes)),
                Ask::Board(market_id) => Commodity::fetch_all(&db, *market_id)
                    .await
                    .map(|board| Answer::Board(*market_id, board)),
                Ask::Trades(Some(market_id), filters) => {
                    Trade::from_market(&db, *market_id, filters)
                        .await
                        .map(|found| Answer::Trades(Some(*market_id), found))
                }
                Ask::Trades(None, filters) => Trade::anywhere(&db, filters)
                    .await
                    .map(|found| Answer::Trades(None, found)),
                Ask::Near(origin, filters) => Trade::near(&db, origin, filters)
                    .await
                    .map(|found| Answer::Near(origin.clone(), found)),
                // Two questions of one market pair, answered together so the
                // pane never draws a comparison beside somebody else's
                // distance.
                Ask::Compare(here, there) => {
                    match Comparison::fetch_all(&db, *here, *there).await {
                        Ok(rows) => between(&db, *here, *there)
                            .await
                            .map(|ly| Answer::Compare(*here, *there, rows, ly)),
                        Err(err) => Err(err),
                    }
                }
            };

            // Nobody is left to hear it once the window has gone, which is
            // the only way this send fails and is not worth saying anything
            // about.
            let _ = home.send(match answer {
                Ok(answer) => answer,
                Err(err) => Answer::Trouble(ask, err.to_string()),
            });
        });
    }

    /// Every answer that has arrived since this was last called
    pub fn answers(&mut self) -> Vec<Answer> {
        let mut arrived = Vec::new();
        loop {
            match self.answers.try_recv() {
                Ok(answer) => {
                    self.outstanding = self.outstanding.saturating_sub(1);
                    arrived.push(answer);
                }
                // The sender is held here as well as by the tasks, so the
                // channel cannot be disconnected while this exists.
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        arrived
    }

    /// How many questions are still out
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }
}
