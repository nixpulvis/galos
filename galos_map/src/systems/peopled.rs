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

use crate::systems::MapSet;
use crate::{Populated, ResidentIndex};
use bevy::prelude::*;
use galos_index::CellId;
use rustc_hash::FxHashMap;
use std::ops::Range;

pub fn plugin(app: &mut App) {
    app.init_resource::<Peopled>();
    // Before anything draws from it, and only when the tables it is built
    // from arrive — which is once, at startup.
    app.add_systems(
        Update,
        gather
            .in_set(MapSet::Populate)
            .before(crate::systems::bounded::reconcile),
    );
}

/// The systems anybody lives in, by the cell that holds them, busiest first
///
/// **A flat array and a range apiece, not a list per cell.** Every
/// populated system belongs to every cell on its path from the root, so
/// the entries outnumber the systems: measured over `.index/full`,
/// 1,669,602 entries over 148,199 systems and **4,314 cells** — the tree
/// is shallow where the colonies are, thirteen levels at the deepest. That
/// is 13.4 MB of addresses and a hundred kilobytes of ranges, gathered in
/// 24 ms at startup, against the whole-cell payload reads it replaces: one
/// faction filter at twelve thousand light years held **2.0 GB** of
/// payload where the same view unfiltered held 46 MB.
///
/// Sorted busiest first within each cell, and by address where two are
/// equal, so the order is the same answer every time rather than whatever
/// the sort happened to do.
#[derive(Resource, Default)]
pub struct Peopled {
    /// Addresses, a cell's own run at a time.
    lived: Vec<i64>,
    /// Where each cell's run sits in it.
    runs: FxHashMap<CellId, Range<u32>>,
}

impl Peopled {
    /// The systems this cell's subtree holds that anybody lives in,
    /// busiest first.
    ///
    /// Empty for a cell nobody lives in, which is nearly every cell: the
    /// map holds two hundred million systems and 148,199 of them are
    /// inhabited.
    pub fn of(&self, id: CellId) -> &[i64] {
        match self.runs.get(&id) {
            Some(run) => &self.lived[run.start as usize..run.end as usize],
            None => &[],
        }
    }

    /// How many cells anybody lives in.
    #[cfg(test)]
    fn cells(&self) -> usize {
        self.runs.len()
    }
}

/// Gather the populated table into the cells that hold it
///
/// Once, when the index and the table have both arrived. The descent is
/// the tree's own — [`galos_index::Index::descend`] — so a system lands in
/// exactly the cells the walk can mark, and a cell asked about its people
/// gets the whole of its subtree's rather than the slice it happens to
/// own.
pub(crate) fn gather(
    index: Res<ResidentIndex>,
    populated: Res<Populated>,
    mut peopled: ResMut<Peopled>,
) {
    if !index.is_changed() && !populated.is_changed() {
        return;
    }
    let _zone = info_span!("gathering the peopled").entered();

    // Gathered per cell and then flattened, since a system is met once per
    // cell of its path and the paths interleave. The population rides
    // along to sort by and is dropped: what the draw wants is the order.
    let mut gathered: FxHashMap<CellId, Vec<(u64, i64)>> = FxHashMap::default();
    for system in populated.0.values() {
        if system.population == 0 {
            continue;
        }
        let at = [
            f64::from(system.position[0]),
            f64::from(system.position[1]),
            f64::from(system.position[2]),
        ];
        index.0.descend(at, |id| {
            gathered
                .entry(id)
                .or_default()
                .push((system.population, system.address));
        });
    }

    let mut lived: Vec<i64> = Vec::with_capacity(
        gathered.values().map(Vec::len).sum::<usize>(),
    );
    let mut runs: FxHashMap<CellId, Range<u32>> =
        FxHashMap::with_capacity_and_hasher(gathered.len(), Default::default());
    for (id, mut people) in gathered {
        people.sort_unstable_by_key(|&(count, address)| {
            (std::cmp::Reverse(count), address)
        });
        let from = lived.len() as u32;
        lived.extend(people.into_iter().map(|(_, address)| address));
        runs.insert(id, from..lived.len() as u32);
    }

    info!(
        cells = runs.len(),
        systems = lived.len(),
        "gathered who lives where",
    );
    *peopled = Peopled { lived, runs };
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_index::meta::PopulatedSystem;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// One populated system at `at` with `population` living there.
    fn lived_in(address: i64, at: [f32; 3], population: u64) -> PopulatedSystem {
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
        app.init_resource::<Peopled>();
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

    /// A cell answers with the people of its whole subtree, busiest first
    ///
    /// Busiest first because that is the order the marks are spent in: a
    /// cell draws a share of what it holds, and in this mode a mark's size
    /// is how many live there, so the ones worth drawing are the large.
    #[test]
    fn a_cell_answers_with_its_people_busiest_first() {
        let app = gathered(vec![
            lived_in(1, [0., 0., 0.], 1_000),
            lived_in(2, [1., 0., 0.], 40_000),
            lived_in(3, [2., 0., 0.], 9_000),
        ]);
        let peopled = app.world().resource::<Peopled>();

        // The root holds all three, busiest first.
        assert_eq!(peopled.of(CellId::ROOT), &[2, 3, 1]);
        // And every one of them is in the deepest cell that holds it.
        for (address, at) in
            [(1i64, [0., 0., 0.]), (2, [1., 0., 0.]), (3, [2., 0., 0.])]
        {
            let deepest = CellId::of_point(at, 13);
            let mut held = false;
            let mut id = deepest;
            loop {
                held |= peopled.of(id).contains(&address);
                match id.parent() {
                    Some(up) => id = up,
                    None => break,
                }
            }
            assert!(held, "nothing holds {address}");
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
        let peopled = app.world().resource::<Peopled>();
        assert_eq!(peopled.of(CellId::ROOT), &[2]);
    }

    /// Nothing is gathered twice when the tables have not moved: it is a
    /// pass over every populated system, and it belongs at startup.
    #[test]
    fn it_is_gathered_once() {
        let mut app = gathered(vec![lived_in(1, [0., 0., 0.], 5)]);
        let before = app.world().resource::<Peopled>().cells();
        app.update();
        app.update();
        let after = app.world().resource::<Peopled>();
        assert_eq!(after.cells(), before);
        assert_eq!(after.of(CellId::ROOT), &[1], "gathered twice over");
    }
}
