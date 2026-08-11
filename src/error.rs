//! What a query can fail with
//!
//! Separate from [`galos_db::Error`] because half of what goes wrong here is
//! not the database going wrong. A name nothing is called and a route that
//! does not exist are both perfectly good answers to a perfectly good
//! question, and reporting them as `RowNotFound` tells the user about our
//! query builder instead of about their galaxy.
//!
//! The terminal UI shows these on the page that failed and stays where it is,
//! so they are written to be read by somebody who is still looking at the
//! thing they asked about.

use std::{error, fmt};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// The database could not be asked, or did not answer
    Db(galos_db::Error),
    /// Nothing is called that
    Unknown { thing: &'static str, name: String },
    /// It is on record, but not anywhere
    ///
    /// Roughly three quarters of the systems held have no coordinates, Sol
    /// among them. Routing to one is impossible, and saying so is not the
    /// same as saying it does not exist.
    Unplaced { name: String },
    /// Both ends exist and nothing joins them
    NoRoute { start: String, end: String, range: f64 },
    /// The query was asked for in a way that cannot be answered
    ///
    /// Arguments clap will take that mean nothing together, which is where
    /// the shared parser stops being able to help: it can require a string
    /// and cannot require that a radius be measured from one system rather
    /// than from a pattern matching four hundred.
    Nonsense(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Db(e) => write!(f, "{}", e),
            Error::Unknown { thing, name } => {
                write!(f, "no {thing} named {name}")
            }
            Error::Unplaced { name } => {
                write!(f, "{name} has no position on record")
            }
            Error::NoRoute { start, end, range } => write!(
                f,
                "no route from {start} to {end} in jumps of {range:.2} Ly"
            ),
            Error::Nonsense(what) => write!(f, "{what}"),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Error::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<galos_db::Error> for Error {
    fn from(err: galos_db::Error) -> Error {
        Error::Db(err)
    }
}
