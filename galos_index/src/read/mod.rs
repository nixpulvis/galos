//! The reading side: planning a [`View`](walk::View) on the resident
//! [`Index`](crate::tree::index::Index) and fetching what the plan wants
//! through a [`Source`](source::Source); a router's [`Sky`](sky::Sky) besides.
//!
//! [`crate::tree::index`] is the resident cell tree and [`walk`] the
//! traversals that plan a frame on it, which [`screen`] rations down to what
//! is drawn and [`resident`] holds loaded. [`sky`] is the galaxy's places for
//! a router, [`inhabited`] the populated few rolled up the tree, and
//! [`source`] the one seam every read goes through.

pub mod inhabited;
pub mod resident;
pub mod screen;
pub mod sky;
pub mod source;
pub mod walk;
