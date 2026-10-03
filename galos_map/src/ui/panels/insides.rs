//! What is inside the systems a panel is open on: their stars and bodies

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use galos_index::records::SystemBodies;
use std::collections::HashMap;

/// The stars and bodies of each system a panel stands open on
///
/// **Read for the panel, not for the map.** [`crate::map::bodies::Contents`]
/// holds the insides of the one system the camera is in, and a panel is
/// opened on whatever system the user picked out, usually somewhere else.
/// So each open system's record is read once, off the pool, the same file
/// [`crate::map::bodies`] reads when the camera arrives.
///
/// Let go of when the last panel on a system shuts, so a panel opened again
/// reads it afresh rather than saying what was on record an hour ago.
#[derive(Resource, Default)]
pub struct Insides {
    known: HashMap<i64, SystemBodies>,
    reading: HashMap<i64, Task<SystemBodies>>,
}

impl Insides {
    /// What is on record inside `address`, [`None`] while it is being read
    pub(super) fn of(&self, address: i64) -> Option<&SystemBodies> {
        self.known.get(&address)
    }

    /// Read whatever of `open` has not been, take in what has landed, and
    /// let go of every system no panel stands open on
    pub(super) fn keep(
        &mut self,
        open: &[i64],
        transport: &crate::map::index::Transport,
    ) {
        self.known.retain(|address, _| open.contains(address));
        self.reading.retain(|address, _| open.contains(address));
        self.reading.retain(|address, task| {
            match bevy::tasks::block_on(
                bevy::tasks::futures_lite::future::poll_once(task),
            ) {
                Some(inside) => {
                    self.known.insert(*address, inside);
                    false
                }
                None => true,
            }
        });
        for &address in open {
            if self.known.contains_key(&address)
                || self.reading.contains_key(&address)
            {
                continue;
            }
            let reading = transport.0.clone();
            // A system the source cannot speak about and one with nothing on
            // record read the same: nothing scanned.
            let task = AsyncComputeTaskPool::get().spawn(async move {
                reading.bodies(address).await.unwrap_or_default()
            });
            self.reading.insert(address, task);
        }
    }
}
