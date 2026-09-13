//! Whether a run has been asked to stop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Whether the run has been asked to stop.
///
/// Read by every loop in the program and set by the SIGINT handler. A flag
/// rather than a channel because that is the whole of what it has to carry:
/// nothing is published through it, so a loop that reads it one pass late
/// does one more pass, which is the point — what must not happen is a loop
/// that never reads it, or a process that dies before its sink is closed.
#[derive(Clone, Default)]
pub struct Shutdown(Arc<AtomicBool>);

impl Shutdown {
    pub fn new() -> Shutdown {
        Shutdown::default()
    }

    /// Ask the run to stop at the next place it can.
    ///
    /// The signal handler, and the index worker where it has failed: a half a
    /// run is not a run, and the sources would otherwise read a feed that
    /// never ends into a channel nobody is draining.
    pub fn ask(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether it has been asked.
    pub fn asked(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
