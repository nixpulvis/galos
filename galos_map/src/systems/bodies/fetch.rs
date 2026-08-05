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
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_db::bodies::Body as DbBody;
use galos_db::stars::Star as DbStar;

pub fn plugin(app: &mut App) {
    app.init_resource::<Asking>();
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

/// The query in flight, if there is one
///
/// Separate from [`Contents`] so that dropping the task cancels it: a system
/// left behind while its rows are still coming back should not land them on
/// the map a moment later.
#[derive(Resource, Default)]
struct Asking(Option<(i64, Task<Answer>)>);

/// What the database had about one system
struct Answer {
    stars: Vec<DbStar>,
    bodies: Vec<DbBody>,
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
    mut contents: ResMut<Contents>,
    mut asking: ResMut<Asking>,
) {
    let Ok(focus) = camera.single().map(|camera| camera.focus) else { return };

    let nearest = systems
        .iter()
        .map(|system| {
            (system.address, focus.distance(DVec3::from(system.position)))
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
                && focus.distance(DVec3::from(system.position))
                    <= HOLD_WITHIN as f64
        })
    {
        return;
    }

    let Some(address) = nearest else {
        // Out of reach of anything. Let go of what was held, and of any
        // question still outstanding about it.
        if contents.of().is_some() {
            *contents = Contents::default();
            asking.0 = None;
        }
        return;
    };
    if contents.of() == Some(address) {
        return;
    }

    debug!("asking what is in {address}");
    let pool = AsyncComputeTaskPool::get();
    let db = db.0.clone();
    let task = pool.spawn(async move {
        // Nothing is made of a failure but an empty answer. A system the
        // database cannot speak about and one it has nothing to say about
        // are the same thing to a map that has to draw something either way.
        Answer {
            stars: DbStar::fetch_all(&db, address).await.unwrap_or_default(),
            bodies: DbBody::fetch_all(&db, address).await.unwrap_or_default(),
        }
    });

    *contents = Contents { of: Some(address), held: Held::Asking };
    // Replaces whatever was outstanding, and dropping it is what cancels it.
    asking.0 = Some((address, task));
}

/// Take in whatever has come back
fn collect(mut contents: ResMut<Contents>, mut asking: ResMut<Asking>) {
    let Some((address, task)) = asking.0.as_mut() else { return };
    let Some(answer) = block_on(future::poll_once(task)) else { return };
    let address = *address;
    asking.0 = None;

    // The camera may have moved on while this was in flight, in which case
    // the answer is about somewhere nobody is standing any more.
    if contents.of() != Some(address) {
        return;
    }

    debug!(
        "{address} holds {} stars and {} bodies",
        answer.stars.len(),
        answer.bodies.len()
    );
    contents.held = Held::Known { stars: answer.stars, bodies: answer.bodies };
}
