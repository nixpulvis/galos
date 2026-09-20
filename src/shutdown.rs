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

/// Ask the run to stop on Ctrl-C, and kill it on the second one.
///
/// The whole reason there is a handler at all: the default action for
/// SIGINT is to die where it stands, and where it stands is usually
/// mid-publish, with a directory serving systems no resume point knows
/// about. Asking instead costs whatever is left of the current publish. A
/// second Ctrl-C is somebody who has decided that is too long, and it is
/// theirs to have — the directory is what it was before the run started
/// publishing over it.
///
/// Here rather than in a binary because both ingests want it, and a
/// handler that half the tools install is a tool that kills a directory.
pub fn on_interrupt(shutdown: Shutdown) {
    let installed = ctrlc::set_handler(move || {
        if shutdown.asked() {
            eprintln!("stopping now; what is being written may be half done");
            std::process::exit(130);
        }
        eprintln!(
            "stopping: the last publish and the resume point still have to \
             be written, so this takes a moment. Ctrl-C again to stop now."
        );
        shutdown.ask();
    });
    if let Err(err) = installed {
        tracing::warn!(
            error = %err,
            "no signal handler; Ctrl-C will kill this run mid-publish",
        );
    }
}
