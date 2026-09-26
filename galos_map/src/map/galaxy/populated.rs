//! Who lives in each cell, off the table the map already holds.
//!
//! **The populated systems are resident in full and always were.** The
//! index publishes `populated.bin` — every system anybody lives in, with
//! its place, its population and its politics, 148,199 rows and 8.7 MB
//! over `.index/full` — and the client loads all of it at startup. What
//! was missing is the one question the draw asks of it: *which of them are
//! in this cell*.
//!
//! **Without that the population scale drew the wrong sky.** A cell's
//! payload is magnitude-ordered and read as a prefix sized for the mark
//! count, and one system in forty-four is populated, scattered anywhere
//! through it — so picking the busiest out of the prefix picked the
//! busiest of an arbitrary head of the cell. What that showed depended on
//! where the camera had been: measured over `.index/full` from 2,300,
//! 13, 5,300 light years, a settled view two thousand light years back
//! drew **57 systems arriving and 92 after a zoom in and back out**, the
//! zoom having read the cells whole and left them resident. The same view,
//! two answers.
//!
//! `fetch`'s own doc had the rule and the population scale was not counted
//! under it: *the filters promote systems out of magnitude order, so a
//! prefix is the one thing that cannot answer them*. Drawing by population
//! is that same promotion.
//!
//! So it is answered here instead, from the resident table, and no payload
//! is read for it at all. Exact, the same however the eye arrived, and
//! cheaper than the reads it replaces.

use crate::map::galaxy::MapSet;
use crate::map::index::{Populated, ResidentIndex};
use bevy::prelude::*;
use galos_index::CellId;

pub fn plugin(app: &mut App) {
    app.init_resource::<PopulatedOrder>();
    // Before anything draws from it, and only when the tables it is built
    // from arrive — which is once, at startup.
    app.add_systems(
        Update,
        gather
            .in_set(MapSet::Populate)
            .before(crate::map::galaxy::walk::reconcile),
    );
}

/// Every system anybody lives in, busiest first
///
/// **One order for the galaxy, not a list per cell.** A draw wants the
/// busiest systems in reach, and a cell is not how it wants to ask:
/// every populated system belongs to every cell on its path from the
/// root, so a pass that asks cell by cell walks the same systems once
/// per level — measured over `.index/full` with the reach at five
/// hundred light years, 1,559,152 entries read to choose 25,744 marks,
/// over an index holding 148,199 populated systems in all. Read as one
/// order it is 148,199 entries, and the pass stops as soon as the sky
/// is full.
///
/// 7.1 MB — 48 bytes apiece — gathered in 18 ms at startup, against the
/// whole-cell payload reads it replaces: one faction filter at twelve
/// thousand light years held **2.0 GB** of payload where the same view
/// unfiltered held 46 MB.
///
/// Busiest first, and by address where two are equal, so it is the same
/// answer every time rather than whatever the sort happened to do.
#[derive(Resource, Default)]
pub struct PopulatedOrder {
    order: Vec<Stands>,
}

/// One populated system as a draw wants it: where it stands, what to
/// call it up by, and the deepest cell of the tree that holds it
///
/// The cell because the draw books its mark against one — the field
/// subtracts what a cell has drawn from the light it lays for that cell,
/// so a mark nobody accounts for is a mark drawn twice. The deepest, and
/// the draw walks up from it to the shallowest the plan marks: a dozen
/// steps, and only for what is drawn.
#[derive(Copy, Clone, Debug)]
pub struct Stands {
    pub address: i64,
    pub at: [f64; 3],
    pub deepest: CellId,
}

impl PopulatedOrder {
    /// Every populated system, busiest first.
    pub fn order(&self) -> &[Stands] {
        &self.order
    }

    /// How many systems anybody lives in.
    #[cfg(test)]
    fn systems(&self) -> usize {
        self.order.len()
    }
}

/// Put the populated table in the order a draw reads it
///
/// Once, when the index and the table have both arrived. The cell each
/// lands in is found by the tree's own descent —
/// [`galos_index::Index::descend`] — so it is a cell the walk can mark
/// rather than one computed beside it.
pub(crate) fn gather(
    index: Res<ResidentIndex>,
    populated: Res<Populated>,
    mut cells: ResMut<PopulatedOrder>,
) {
    if !index.is_changed() && !populated.is_changed() {
        return;
    }
    let _zone = info_span!("gathering the populated").entered();

    // The population rides along to sort by and is then dropped: what a
    // draw wants out of this is the order, and re-reading the count off
    // the table costs a hash lookup it never needs.
    let mut order: Vec<(u64, Stands)> = Vec::new();
    for system in populated.0.values() {
        if system.population == 0 {
            continue;
        }
        let at = [
            f64::from(system.position[0]),
            f64::from(system.position[1]),
            f64::from(system.position[2]),
        ];
        let mut deepest = CellId::ROOT;
        index.0.descend(at, |id| deepest = id);
        order.push((
            system.population,
            Stands { address: system.address, at, deepest },
        ));
    }
    order.sort_unstable_by_key(|&(count, stands)| {
        (std::cmp::Reverse(count), stands.address)
    });
    let order: Vec<Stands> =
        order.into_iter().map(|(_, stands)| stands).collect();

    info!(systems = order.len(), "gathered who lives where");
    *cells = PopulatedOrder { order };
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_index::records::PopulatedSystem;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// One populated system at `at` with `population` living there.
    fn lived_in(
        address: i64,
        at: [f32; 3],
        population: u64,
    ) -> PopulatedSystem {
        PopulatedSystem {
            address,
            name: format!("System {address}").into(),
            position: at,
            population,
            security: None,
            government: None,
            allegiance: None,
            primary_economy: None,
            secondary_economy: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
        }
    }

    /// A world holding an index over `at` and the populated table beside it.
    fn gathered(rows: Vec<PopulatedSystem>) -> App {
        use galos_index::{BuildParams, Snapshot, StarKind};
        let systems: Vec<galos_index::System> = rows
            .iter()
            .map(|row| galos_index::System {
                id64: row.address as u64,
                position: [
                    f64::from(row.position[0]),
                    f64::from(row.position[1]),
                    f64::from(row.position[2]),
                ],
                absolute_magnitude: 4.,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: StarKind::Unknown,
            })
            .collect();
        let built = Snapshot::build(
            &systems,
            &BuildParams { internal_slice: 1, leaf_cap: 1 },
        );

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<PopulatedOrder>();
        app.insert_resource(ResidentIndex(built.index.clone()));
        app.insert_resource(Populated(Arc::new(
            rows.into_iter()
                .map(|row| (row.address, row))
                .collect::<HashMap<_, _>>(),
        )));
        app.add_systems(Update, gather);
        app.update();
        app
    }

    /// The table answers busiest first
    ///
    /// Busiest first because that is the order the marks are spent in: a
    /// draw takes from the front until the sky is full, and in this mode a
    /// mark's size is how many live there, so the ones worth drawing are
    /// the large.
    #[test]
    fn the_populated_answer_busiest_first() {
        let app = gathered(vec![
            lived_in(1, [0., 0., 0.], 1_000),
            lived_in(2, [1., 0., 0.], 40_000),
            lived_in(3, [2., 0., 0.], 9_000),
        ]);
        let held = app.world().resource::<PopulatedOrder>();

        let order: Vec<i64> =
            held.order().iter().map(|stands| stands.address).collect();
        assert_eq!(order, &[2, 3, 1]);
    }

    /// Each is carried with the deepest cell of the tree that holds it,
    /// which is what a draw books its mark against.
    #[test]
    fn each_knows_the_deepest_cell_that_holds_it() {
        let app = gathered(vec![
            lived_in(1, [0., 0., 0.], 1_000),
            lived_in(2, [1., 0., 0.], 40_000),
        ]);
        let held = app.world().resource::<PopulatedOrder>();

        for stands in held.order() {
            assert!(
                stands.deepest.level > 0,
                "{} was left at the root",
                stands.address,
            );
            assert_eq!(
                stands.deepest,
                CellId::of_point(stands.at, stands.deepest.level),
                "{} is not in the cell it was given",
                stands.address,
            );
        }
    }

    /// A system nobody lives in is not gathered, that mode drawing none of
    /// them at all.
    #[test]
    fn an_empty_system_is_nobody() {
        let app = gathered(vec![
            lived_in(1, [0., 0., 0.], 0),
            lived_in(2, [1., 0., 0.], 7),
        ]);
        let held = app.world().resource::<PopulatedOrder>();
        let order: Vec<i64> =
            held.order().iter().map(|stands| stands.address).collect();
        assert_eq!(order, &[2]);
    }

    /// Nothing is gathered twice when the tables have not moved: it is a
    /// pass over every populated system, and it belongs at startup.
    #[test]
    fn it_is_gathered_once() {
        let mut app = gathered(vec![lived_in(1, [0., 0., 0.], 5)]);
        let before = app.world().resource::<PopulatedOrder>().systems();
        app.update();
        app.update();
        let after = app.world().resource::<PopulatedOrder>();
        assert_eq!(after.systems(), before);
        assert_eq!(after.order()[0].address, 1, "gathered twice over",);
    }
}
