//! Asking the database what is inside a system
//!
//! Its own task rather than another arm of [`crate::systems::fetch`], which
//! answers rows of `systems` and only those. A second kind of answer on the
//! same map would make it a map of two unrelated things keyed alike.

use super::{Contents, Held};
use crate::Db;
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::fetch::Poll;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_db::barycenters::Barycenter as DbBarycenter;
use galos_db::bodies::Body as DbBody;
use galos_db::stars::Star as DbStar;
use std::time::Instant;

pub fn plugin(app: &mut App) {
    app.init_resource::<Polling>();
    app.add_systems(Update, choose.in_set(MapSet::Fetch));
    app.add_systems(Update, collect.in_set(MapSet::Populate));
}

/// How near a system has to be for the map to ask what is in it, in light
/// years
///
/// Far enough out that the shell is still nearly all of the angle that keeps
/// it visible, so that exchanging the size the map assumed for the size the
/// system is barely moves it. Nearer than this and the exchange begins to
/// show; further and the nearest system changes often enough to be asked
/// after repeatedly.
///
/// Nothing about what is drawn waits on this. The rows are held, and the
/// bodies, the grid and the camera's descent all happen far closer in, so
/// this is a question about having the answer in hand rather than about
/// having anything on screen.
const ASK_WITHIN: f32 = 5.;

/// How far the camera may drift before the map lets a system go, in light
/// years
///
/// Wider than [`ASK_WITHIN`], so that a camera sitting on the line between two
/// systems does not spend every frame swapping which of them is held. What is
/// held is dropped only once something else is clearly nearer.
const HOLD_WITHIN: f32 = 7.;

/// What is outstanding, and when it was asked
#[derive(Resource, Default)]
pub(super) struct Polling {
    /// The query in flight, if there is one
    ///
    /// Separate from [`Contents`] so that dropping the task cancels it: a
    /// system left behind while its rows are still coming back should not land
    /// them on the map a moment later.
    query: Option<(i64, Task<Answer>)>,
    /// When the system being held was last asked about
    ///
    /// What [`Poll`] is measured from, and nothing until something has been
    /// asked.
    asked_at: Option<Instant>,
}

impl Polling {
    /// Whether what is held is due to be asked about again
    ///
    /// On the same [`Poll`] the spyglass refreshes systems on, that being the
    /// same question put about the inside of one: how often should the map ask
    /// again for what it already has. So the checkbox beside it holds a system
    /// still, and the box holds both to one answer.
    ///
    /// Never while a query is outstanding, so a poll landing on top of one
    /// still on the wire is a poll that does nothing rather than a second copy
    /// of the same answer. Measured from when the last was asked rather than
    /// when it came back, as the spyglass measures it.
    fn due(&self, poll: &Poll, now: Instant) -> bool {
        self.query.is_none()
            && self.asked_at.is_none_or(|last| poll.elapsed(last, now))
    }

    /// Put the question about `address`, dropping whatever was outstanding
    fn ask(&mut self, db: &Db, address: i64, now: Instant) {
        let db = db.0.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            // Nothing is made of a failure but an empty answer. A system the
            // database cannot speak about and one it has nothing to say about
            // are the same thing to a map that has to draw something either
            // way.
            Answer {
                stars: DbStar::fetch_all(&db, address)
                    .await
                    .unwrap_or_default(),
                bodies: DbBody::fetch_all(&db, address)
                    .await
                    .unwrap_or_default(),
                centers: DbBarycenter::fetch_all(&db, address)
                    .await
                    .unwrap_or_default(),
            }
        });

        self.query = Some((address, task));
        self.asked_at = Some(now);
    }
}

/// What the database had about one system
pub(super) struct Answer {
    stars: Vec<DbStar>,
    bodies: Vec<DbBody>,
    /// The points a close pair goes round
    ///
    /// Asked for with the rest because a body naming one as its nearest
    /// ancestor cannot be placed without it: the walk back to the star stops
    /// at whatever is missing, and a pair whose center is missing is a pair
    /// drawn at the middle of the system. The ellipse the pair rides is drawn
    /// from the same row.
    centers: Vec<DbBarycenter>,
}

/// Decide which system the map is standing in, and ask about it
///
/// The nearest to what the camera is looking at, which is where the user has
/// put themselves. One at a time: the contents of a system are only worth
/// having when it is about to be flown into, and only one can be.
fn choose(
    camera: Query<&OrbitCamera>,
    systems: Query<&System>,
    db: Res<Db>,
    time: Res<Time<Real>>,
    poll: Res<Poll>,
    mut contents: ResMut<Contents>,
    mut polling: ResMut<Polling>,
) {
    let Ok(center) = camera.single().map(|camera| camera.center) else {
        return;
    };
    let now = time.last_update().unwrap_or(time.startup());

    let nearest = systems
        .iter()
        .map(|system| {
            (system.address, center.distance(DVec3::from(system.position)))
        })
        .filter(|(_, away)| *away <= ASK_WITHIN as f64)
        .min_by(|(_, one), (_, other)| one.total_cmp(other))
        .map(|(address, _)| address);

    // What is held stays held while it is anywhere near, so that standing
    // between two systems does not swap between them every frame. Only
    // something else coming within reach displaces it.
    if let Some(held) = contents.of()
        && nearest.is_none_or(|near| near == held)
        && systems.iter().any(|system| {
            system.address == held
                && center.distance(DVec3::from(system.position))
                    <= HOLD_WITHIN as f64
        })
    {
        // A system is scanned while the camera stands in it, and the rows land
        // from another program entirely. Nothing says they arrived, so what is
        // held is asked after again on the poll.
        if polling.due(&poll, now) {
            debug!("asking again what is in {held}");
            polling.ask(&db, held, now);
        }
        return;
    }

    let Some(address) = nearest else {
        // Out of reach of anything. Let go of what was held, and of any
        // question still outstanding about it.
        if contents.of().is_some() {
            *contents = Contents::default();
            *polling = Polling::default();
        }
        return;
    };
    if contents.of() == Some(address) {
        if polling.due(&poll, now) {
            debug!("asking again what is in {address}");
            polling.ask(&db, address, now);
        }
        return;
    }

    debug!("asking what is in {address}");
    *contents = Contents { of: Some(address), held: Held::Asking, revision: 0 };
    polling.ask(&db, address, now);
}

/// Take in whatever has come back
pub(super) fn collect(
    mut contents: ResMut<Contents>,
    mut polling: ResMut<Polling>,
) {
    let Some((address, task)) = polling.query.as_mut() else { return };
    let Some(answer) = block_on(future::poll_once(task)) else { return };
    let address = *address;
    polling.query = None;

    // The camera may have moved on while this was in flight, in which case
    // the answer is about somewhere nobody is standing any more.
    if contents.of() != Some(address) {
        return;
    }

    debug!(
        "{address} holds {} stars, {} bodies and {} barycenters",
        answer.stars.len(),
        answer.bodies.len(),
        answer.centers.len()
    );
    contents.know(answer.stars, answer.bodies, answer.centers);
}
