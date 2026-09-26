//! Whether a run has been asked to stop.

use std::io::Write;
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

/// Ask the run to stop on an interrupt, and kill it on the second one.
///
/// The whole reason there is a handler at all: the default action for
/// SIGINT is to die where it stands, and where it stands is usually
/// mid-publish, with a directory serving systems no resume point knows
/// about. Asking instead costs whatever is left of the current publish. A
/// second interrupt is somebody who has decided that is too long, and it
/// is theirs to have — the directory is what it was before the run started
/// publishing over it.
///
/// **SIGTERM and SIGHUP as well as SIGINT**, which is the `termination`
/// feature of `ctrlc` and is how a run with nobody at a terminal is
/// stopped: `systemctl stop`, `docker stop` and a plain `kill` all send
/// TERM. Taking only Ctrl-C meant a service manager killed the ingest
/// mid-publish and left `<dir>.lock` behind, so the restart it was on its
/// way to refused the directory until somebody passed `--force-lock`.
///
/// Here rather than in a binary because both ingests want it, and a
/// handler that half the tools install is a tool that kills a directory.
pub fn on_interrupt(shutdown: Shutdown) {
    let installed = ctrlc::set_handler(move || {
        // Written with `let _`, never `eprintln!`: a SIGHUP is what a closed
        // terminal sends, and by then stderr is revoked. `eprintln!` panics
        // on that, which here kills the handler thread before the run is
        // asked to stop — leaving a writer nobody can see that ignores every
        // signal after, still holding the directory.
        let say = |said: &str| {
            let _ = writeln!(std::io::stderr(), "{said}");
        };
        if shutdown.asked() {
            say("stopping now; what is being written may be half done");
            std::process::exit(130);
        }
        shutdown.ask();
        say(
            "stopping: the last publish and the resume point still have to \
             be written, so this takes a moment. Interrupt again to stop now.",
        );
    });
    if let Err(err) = installed {
        tracing::warn!(
            error = %err,
            "no signal handler; an interrupt will kill this run mid-publish",
        );
    }
}
