//! Asking the database what is inside a system
//!
//! Its own task rather than another arm of [`crate::systems::fetch`], which
//! answers rows of `systems` and only those. A second kind of answer on the
//! same map would make it a map of two unrelated things keyed alike.

use super::{Contents, FetchState};
use crate::Transport;
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::bodies::spawn::Strength;
use crate::systems::fetch::Poll;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use galos_index::meta::SystemBodies;
use std::time::Instant;

pub fn plugin(app: &mut App) {
    app.init_resource::<Polling>();
    app.init_resource::<Approaching>();
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
/// Held wider than [`ASK_WITHIN`], so that a camera sitting on the line between
/// two systems does not spend every frame swapping which of them is held. What
/// is held is dropped only once something else is clearly nearer. Narrower and
/// the hysteresis runs backwards: a system would be let go of before anything
/// else was near enough to be asked about.
const HOLD_WITHIN: f32 = 7.;

/// Which of `systems` the map should be holding
///
/// Whichever is nearest what the camera is looking at, each given as whatever
/// the caller needs of it and how far off it is.
///
/// Nearest rather than largest in the sky. A system is drawn as itself once it
/// takes up enough of the view, and how much that is follows its own reach, so
/// the widest of them are drawn as themselves from light years off: Alpha
/// Centauri fills more of the sky from Sol than Sol does from a hundredth of a
/// light year out. Asked which system takes up the most, a camera standing on
/// Sol answers Alpha Centauri.
///
/// Generic in what each system is carried as, so the rule that chooses — the
/// nearest inside [`ASK_WITHIN`], and nothing at all past it — can be
/// exercised on bare numbers, with none of [`Approach`] stood up to do it.
/// Production carries one pair: the address to ask about, and the reach and
/// range [`Approaching`] holds for the zoom floor. Answering both at once is
/// the tuple's doing rather than the generic's; the generic is what lets the
/// choosing be read on its own.
fn worth_holding<T>(systems: impl Iterator<Item = (T, f64)>) -> Option<T> {
    systems
        .filter(|(_, away)| *away <= ASK_WITHIN as f64)
        .min_by(|(_, one), (_, other)| one.total_cmp(other))
        .map(|(it, _)| it)
}

/// The system the camera is closing on, as the zoom floor reads it
///
/// The nearest one to what the camera looks at, which is the one the poll asks
/// about and the one the camera is about to be stopped short of if it turns out
/// to have nothing to descend into. Written here because this is where that
/// system is already picked out, once a frame, off a scan the poll makes
/// anyway; read by [`crate::camera`]'s `zoom_floor`.
///
/// Nothing where the camera is out of reach of every system, which is a camera
/// with nothing to be held off by.
#[derive(Resource, Default)]
pub struct Approaching(pub Option<Approach>);

/// What the camera needs of the system it is closing on
///
/// Both halves are needed together. A floor taken off the reach alone held the
/// camera off a system it was nowhere near: `ASK_WITHIN` is five light years
/// wide, so a wide neighbour became the system the floor was read from while
/// the camera stood on another, and a fifth of a light year of reach is a floor
/// twenty-five light years out. How far off it stands is what says whether the
/// zoom is closing on it at all.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Approach {
    /// How far it reaches from its arrival star, in metres
    pub reach: f32,
    /// How far it stands from what the camera is looking at, in light years
    pub away: f32,
}

impl Approach {
    /// Whether the camera is standing in the system rather than beside it
    ///
    /// [`crate::camera`]'s `STOOD_IN`, which is the same distance for every
    /// system, so a wide one does not claim the sky its neighbours stand in.
    /// Not how far the system reaches, and not the band its mark fades over:
    /// the floor this answers for is only ever applied to a system with
    /// nothing to descend into, whose mark never fades, and a fifth of a light
    /// year of reach would otherwise hold the camera off from four light years
    /// away.
    pub fn stood_in(&self) -> bool {
        f64::from(self.away) * crate::space::LIGHT_YEAR
            <= f64::from(crate::camera::STOOD_IN)
    }
}

/// Whether a system already held goes on being held
///
/// A system being closed on is held though the crosshair has left it behind:
/// `standing` under one says its mark is part way out, so its contents, the
/// grid they stand in and the plane the ruler has handed the sky to are all on
/// the map, and letting go of the rows takes all of it away. Otherwise it holds
/// while it is anywhere near, which is what keeps a camera standing between two
/// systems from swapping between them every frame.
///
/// Asked only of a system nothing else is nearer than. Being inside one is
/// *not* enough on its own, however much the fade suggests it:
/// [`super::spawn`]'s `WORTH_MARKING` is an angle, so a system reaching a
/// fifth of a light year has a mark still going out sixteen light years away,
/// and holding the rows on that ground would leave the map unable to descend
/// into a neighbour the camera has been panned right onto. Whichever system
/// the crosshair is nearest is the one worth holding; that the handover has to
/// be an even one is for [`crate::grid`]'s `rule` to answer, not this.
fn holds_still(standing: f32, away: f64) -> bool {
    standing < 1. || away <= HOLD_WITHIN as f64
}

/// What is outstanding, and when it was asked
#[derive(Resource, Default)]
pub(super) struct Polling {
    /// The query in flight, if there is one
    ///
    /// Separate from [`Contents`] so that dropping the task cancels it: a
    /// system left behind while its rows are still coming back should not land
    /// them on the map a moment later.
    query: Option<(i64, Task<SystemBodies>)>,
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
    fn ask(&mut self, transport: &Transport, address: i64, now: Instant) {
        let transport = transport.0.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            // Nothing is made of a failure but an empty answer. A system the
            // source cannot speak about and one it has nothing to say about
            // are the same thing to a map that has to draw something either
            // way.
            transport.bodies(address).await.unwrap_or_default()
        });

        self.query = Some((address, task));
        self.asked_at = Some(now);
    }
}

/// Decide which system the map is standing in, and ask about it
///
/// The nearest to what the camera is looking at, which is where the user has
/// put themselves. One at a time: the contents of a system are only worth
/// having when it is about to be flown into, and only one can be.
fn choose(
    camera: Query<&OrbitCamera>,
    systems: Query<(&System, &Strength)>,
    transport: Res<Transport>,
    time: Res<Time<Real>>,
    poll: Res<Poll>,
    mut contents: ResMut<Contents>,
    mut polling: ResMut<Polling>,
    mut approaching: ResMut<Approaching>,
) {
    let Ok(center) = camera.single().map(|camera| camera.center) else {
        return;
    };
    let now = time.last_update().unwrap_or(time.startup());

    // One pass, answering both which system to ask about and how far it
    // reaches. The reach is the camera's to read: it is what the zoom is
    // stopped short of where the system turns out to have nothing to descend
    // into (see [`Approaching`]).
    let closing = worth_holding(systems.iter().map(|(system, _)| {
        let away = center.distance(system.position());
        let approach = Approach { reach: system.reach(), away: away as f32 };
        ((system.address, approach), away)
    }));
    let nearest = closing.map(|(address, _)| address);
    let closing = closing.map(|(_, approach)| approach);
    if approaching.0 != closing {
        approaching.0 = closing;
    }

    // What is held stays held while nothing else is nearer and either the
    // camera is closing on it or it is anywhere near, so that standing between
    // two systems does not swap between them every frame. See [`holds_still`].
    if let Some(held) = contents.of()
        && nearest.is_none_or(|near| near == held)
        && systems.iter().any(|(system, standing)| {
            system.address == held
                && holds_still(standing.0, center.distance(system.position()))
        })
    {
        // A system is scanned while the camera stands in it, and the rows land
        // from another program entirely. Nothing says they arrived, so what is
        // held is asked after again on the poll.
        if polling.due(&poll, now) {
            debug!("asking again what is in {held}");
            polling.ask(&transport, held, now);
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
            polling.ask(&transport, address, now);
        }
        return;
    }

    debug!("asking what is in {address}");
    *contents =
        Contents { of: Some(address), state: FetchState::Asking, revision: 0 };
    polling.ask(&transport, address, now);
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
        answer.barycenters.len()
    );
    contents.hold(answer.stars, answer.bodies, answer.barycenters);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crosshair says which system the map holds
    ///
    /// Nearest to what the camera is looking at, which is where the user has
    /// put themselves.
    #[test]
    fn the_crosshair_says_which_system_is_held() {
        let near = (1, 0.1);
        let far = (2, 3.);

        assert_eq!(worth_holding([far, near].into_iter()), Some(1));
    }

    /// A wide neighbour does not take the map from where the camera is
    ///
    /// Alpha Centauri reaches a fifth of a light year, so from Sol it fills
    /// three times as much of the sky as Sol's own system does from a
    /// hundredth of a light year out. Held by whichever fills the most, a
    /// camera standing on Sol holds Alpha Centauri, rules the plane about it
    /// and snaps to Sol at the end of the zoom.
    #[test]
    fn a_wide_neighbour_does_not_take_the_map_from_where_the_camera_is() {
        let sol = (1, 0.01);
        let alpha_centauri = (2, 4.4);

        assert_eq!(worth_holding([alpha_centauri, sol].into_iter()), Some(1));
    }

    /// A system is held out further than it is asked about
    ///
    /// Which way round the two reaches go is the whole of the hysteresis. The
    /// other way a system would be let go of before anything else was near
    /// enough to be asked after, and a camera sitting between two of them
    /// would swap which it held every frame.
    #[test]
    fn a_system_is_held_further_out_than_it_is_asked_about() {
        assert!(holds_still(1., ASK_WITHIN as f64));
        assert!(!holds_still(1., HOLD_WITHIN as f64 + 1.));
    }

    /// A system out of reach of the crosshair is no candidate at all
    #[test]
    fn a_system_out_of_reach_is_left_alone() {
        assert_eq!(worth_holding([(1, 9.)].into_iter()), None);
    }

    /// A system being closed on is held though the crosshair has left it behind
    ///
    /// What is drawn hangs off the system being held: its contents, the grid
    /// they stand in and the plane the ruler hands the sky to. Letting go of
    /// one whose mark is still going out takes all of that away mid fade.
    #[test]
    fn a_system_being_closed_on_is_held_from_further_off() {
        assert!(holds_still(0.3, 20.));
        assert!(holds_still(1., 1.));
        assert!(!holds_still(1., 9.));
    }

    /// But a nearer system takes the rows, whatever the fade says
    ///
    /// `holds_still` is asked only of a system nothing else is nearer than,
    /// and that gate is load-bearing. [`super::spawn::WORTH_MARKING`] is an
    /// angle, so a system reaching a fifth of a light year still reads as part
    /// way out sixteen light years off: held on that ground it would keep the
    /// rows against a neighbour the camera had been panned right onto, and the
    /// map could never descend into the neighbour at all.
    #[test]
    fn the_nearest_system_is_the_one_worth_holding() {
        let alpha_centauri = (1, 4.4);
        let sol = (2, 0.);

        assert_eq!(worth_holding([alpha_centauri, sol].into_iter()), Some(2));
    }
}
