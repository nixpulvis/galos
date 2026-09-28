//! What an operator does to a whole directory: [`migrate`](migrate::migrate),
//! [`rewrite`](upgrade::rewrite), [`copy`](copy::copy) and
//! [`absorb`](absorb::absorb).
//!
//! [`migrate`] brings a directory's layout up to date at open, and
//! [`upgrade`] its format when asked. [`copy`] is a backup or a restore, and
//! [`absorb`] folds one directory into another.

pub mod absorb;
pub mod copy;
pub mod migrate;
pub mod upgrade;
