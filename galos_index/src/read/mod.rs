//! The client's side: planning on the resident index and fetching what the
//! plan wants.
//!
//! [`index`] is the resident cell tree and [`walk`] the traversals that plan
//! a frame on it, which [`screen`] rations down to what is drawn and
//! [`resident`] holds loaded. [`sky`] is the galaxy's places for a router,
//! [`inhabited`] the populated few rolled up the tree, and [`source`] the
//! one seam every read goes through.

pub mod index;
pub mod inhabited;
pub mod resident;
pub mod screen;
pub mod sky;
pub mod source;
pub mod walk;
