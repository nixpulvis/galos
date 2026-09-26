//! What an operator does to a whole directory.
//!
//! [`migrate`] brings a directory's layout up to date at open, and
//! [`upgrade`] its format when asked. [`copy`] is a backup or a restore, and
//! [`absorb`] folds one directory into another.

pub mod absorb;
pub mod copy;
pub mod migrate;
pub mod upgrade;
