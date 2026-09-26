//! Where a drive can be supercharged: what a star gives, what a drive takes
//! of it, and the table of where the stars are.
//!
//! The index keeps what a system's arrival star *is* ([`StarKind`]). That a
//! neutron star multiplies a jump by four is a fact about ships, so it is
//! here: [`Boost`] and [`Fsd`] are the router's, and so is the table of
//! boost stars — published beside the index's own as a contributed table
//! ([`BoostTable`]), derived from each system's [`Arrival`], and read back
//! as [`Boosts`].

use galos_index::read::source::{Source, table};
use galos_index::{Arrival, StarKind, Table};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use std::sync::Arc;

/// What a system's arrival star can supercharge a frame shift drive by
///
/// Flying the jet cone of a neutron star or a white dwarf in supercruise, with
/// a fuel scoop, charges the drive for one jump: four times the range off a
/// neutron star, half again off a white dwarf, and more of both off a drive
/// built for it. The charge is held until a jump spends it, so what it is
/// worth is a fact about the system a ship is standing in and not about how it
/// got there — which is what lets the router read it as a property of a place.
///
/// Which of the two, rather than the multiplier: the table is about the sky,
/// and what a boost is worth depends on the drive taking it — see
/// [`Boost::factor`] and [`Fsd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Boost {
    /// A white dwarf: half again, and a much larger exclusion zone to be
    /// caught out by.
    WhiteDwarf,
    /// A neutron star: four times over, which is what a neutron highway is.
    Neutron,
}

impl Boost {
    /// What a star of `kind` can supercharge a drive on, where it can
    ///
    /// Asked of the arrival star, which is the one that matters: a ship drops
    /// in at the main star and can reach its jet cone without crossing the
    /// system. A neutron star is class `N`; every white dwarf class begins
    /// with `D` (`DA`, `DB`, `DC` and their variants). Nothing else has a jet
    /// cone to fly — a black hole is class `H` and gives nothing, whatever it
    /// looks like it should.
    pub fn of(kind: StarKind) -> Option<Boost> {
        match kind {
            StarKind::WhiteDwarf => Some(Boost::WhiteDwarf),
            StarKind::Neutron => Some(Boost::Neutron),
            _ => None,
        }
    }

    /// What a jump out of here is multiplied by, on a drive of `fsd`
    ///
    /// Four times off a neutron star and half again off a white dwarf on a
    /// standard drive; six and three on the Mk II, which is built for it.
    /// The boost is what is worth having and the drive only how much of it
    /// is taken, so the figure is the boost's to say.
    pub fn factor(self, fsd: Fsd) -> f64 {
        match (self, fsd) {
            (Boost::WhiteDwarf, Fsd::MkI) => 1.5,
            (Boost::Neutron, Fsd::MkI) => 4.,
            (Boost::WhiteDwarf, Fsd::MkII) => 3.,
            (Boost::Neutron, Fsd::MkII) => 6.,
        }
    }

    /// The largest factor any boost gives on `fsd`: a neutron star's
    pub fn widest(fsd: Fsd) -> f64 {
        Boost::Neutron.factor(fsd)
    }

    /// What the star is called, for a reader rather than for a router
    ///
    /// The class is all this table keeps of a star — it says what can
    /// supercharge and on what, not a spectrum — so it is the one thing a
    /// client can say about a star's kind without fetching the system's
    /// bodies.
    pub fn named(&self) -> &'static str {
        match self {
            Boost::Neutron => "neutron star",
            Boost::WhiteDwarf => "white dwarf",
        }
    }
}

/// Which frame shift drive takes a jet cone's charge
///
/// What a [`Boost`] is worth depends on it, and nothing else about the ship
/// does. A ship with no drive that can be supercharged is no `Fsd` at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fsd {
    /// A standard frame shift drive: four times off a neutron star, half
    /// again off a white dwarf.
    MkI,
    /// The Mk II Supercharge Optimised drive: six times and three.
    MkII,
}

/// A system whose arrival star can supercharge a drive, where it is, and on
/// what: one row of [`BoostTable`].
///
/// Two systems in a hundred are in it — 3,846,802 of 200,071,629.
///
/// **The place is in it because the router's question is where the cones
/// are.** A table of addresses alone made the client join four million of
/// them against the names table's address column to find out — 4 GB of
/// mapping faulted and 7.9 s before a galactic route could begin planning,
/// paid once a session and visible as a click that hung. Twelve bytes a row
/// here is 46 MB over the table and nothing at all at read time, and it is
/// what takes the names table out of the routing path altogether.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SystemBoost {
    pub address: i64,
    pub boost: Boost,
    /// Where it sits, in light years, at the precision the index's tables
    /// publish: a route resolves a waypoint by descending to the place, so
    /// a light year of rounding changes nothing.
    pub position: [f32; 3],
}

/// The supercharge table, `boosts.bin`: which systems can supercharge a
/// drive, contributed to every index directory the program writes.
///
/// Hand it to whatever writes a directory in a [`galos_index::TableSet`];
/// read it back as [`Boosts`].
pub struct BoostTable;

impl Table for BoostTable {
    const NAME: &'static str = "boosts";
    const ABOUT: &'static str =
        "Which systems can supercharge a drive, which the router plots by.";
    type Row = SystemBoost;

    fn address(row: &SystemBoost) -> i64 {
        row.address
    }

    fn derive(arrival: &Arrival) -> Option<SystemBoost> {
        Some(SystemBoost {
            address: arrival.address,
            boost: Boost::of(arrival.kind)?,
            position: arrival.position,
        })
    }

    /// Give the table the places its rows always implied.
    ///
    /// The table used to be addresses and classes, which left the router to
    /// join four million of them against the names table's address column
    /// to find out where the jet cones are: 4 GB of mapping faulted and 7.9
    /// s before a galactic route could start planning, once a session. The
    /// place belongs in the published row, and this is the one pass that
    /// puts it there — the same join, run once, by the side that publishes.
    ///
    /// A row the payloads cannot place is dropped rather than placed at the
    /// origin, which would put a jet cone at the galactic centre and plan
    /// every route through it. It is the rule the derivations already
    /// follow.
    ///
    /// Not interruptible and it need not be: it is one read of the table,
    /// one pass over the payloads, and one write.
    fn upgrade(dir: &Path) -> io::Result<Option<usize>> {
        /// A row as it was published before it carried a place.
        #[derive(Deserialize)]
        struct Unplaced {
            address: i64,
            boost: Boost,
        }

        let path = galos_index::store::tables::path(dir, Self::NAME);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        // Already placed, which is every table written since. Asked first,
        // so a current directory pays one decode and nothing else.
        if galos_index::store::tables::decode::<BoostTable>(&bytes).is_ok() {
            return Ok(None);
        }
        let mut old: Vec<Unplaced> = rmp_serde::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // The table is published in address order, and the join walks the
        // column in that order; sorted here rather than trusted, since a
        // file that says otherwise would answer places for the wrong
        // systems.
        old.sort_unstable_by_key(|row| row.address);
        let addresses: Vec<i64> = old.iter().map(|row| row.address).collect();

        // **The places come from the payloads, one pass over the galaxy.**
        // A system's place is in the cell that owns it and nowhere else.
        // Asking the tree per address would be a sphere query apiece —
        // milliseconds by four million rows — where the cells hold every
        // place already, in an order this does not care about.
        let index = galos_index::Index::read(dir)?;
        let mut table =
            galos_index::store::tables::Keyed::new(Self::NAME, Self::address);
        for cell in index.cells() {
            for point in galos_index::Index::read_payload(dir, cell.id)? {
                let Ok(which) = addresses.binary_search(&(point.id64 as i64))
                else {
                    continue;
                };
                table.put(SystemBoost {
                    address: old[which].address,
                    boost: old[which].boost,
                    position: [
                        point.pos[0] as f32,
                        point.pos[1] as f32,
                        point.pos[2] as f32,
                    ],
                });
            }
        }
        Ok(Some(table.write(dir)?))
    }
}

/// Which systems can supercharge a drive, where they are, and on what — in
/// address order, as published.
///
/// Two systems in a hundred, held resident because the router weighs it at
/// every step of a search: a route is plotted over the whole galaxy rather
/// than over what is drawn, so a fetch per step is not a thing that could
/// work. What a boost is worth is the drive's to say
/// ([`crate::Drive`]); this is only where one can be had.
///
/// **The published rows themselves**, kept in their own order rather than
/// spread into a map. A `HashMap` over 3.8 M rows is some 140 MB of slots
/// to hold 60 MB of facts, and the order is what the second reader of this
/// table wants: the boost stars are the nodes of
/// [`crate::highway`]'s coarse graph, sorted into cells once and
/// then read where they lie. A lookup is therefore a binary search rather
/// than a hash — asked once per expansion, against a neighbour query that
/// measures hundreds of candidates, so the twenty-odd compares are noise
/// beside it.
///
/// The place comes with the row ([`SystemBoost`]) and that is
/// the whole of why routing no longer touches the names table: finding
/// where four million cones sat used to mean walking the names table's
/// address column, 4 GB of mapping faulted and 7.9 s before a galactic
/// route could start planning.
///
/// Whether the index publishes such a table at all is kept beside it. An
/// index built before the table existed, or one whose builder has not reached
/// it, reads as [`Boosts::absent`] rather than as a galaxy where nobody has a
/// jet cone — and a route for a supercharging ship is then a question the map
/// cannot answer, which the map says rather than answering the unaided
/// one under the supercharged drive's name.
#[derive(Default, Clone)]
pub struct Boosts {
    /// The rows, ascending by address.
    rows: Arc<Vec<SystemBoost>>,
    /// Whether the index published the table this came from
    published: bool,
}

impl Boosts {
    /// What the system at `address` can supercharge, if anything.
    pub fn get(&self, address: i64) -> Option<Boost> {
        let at = self.rows.binary_search_by_key(&address, |row| row.address);
        Some(self.rows[at.ok()?].boost)
    }

    /// The table as the published rows give it.
    ///
    /// Sorted here rather than trusted: the builder writes it in address
    /// order and the lookup is a binary search, which is wrong rather than
    /// slow if a file says otherwise.
    pub fn of(rows: Vec<SystemBoost>) -> Boosts {
        let mut rows = rows;
        if !rows.windows(2).all(|pair| pair[0].address <= pair[1].address) {
            rows.sort_unstable_by_key(|row| row.address);
        }
        Boosts { rows: Arc::new(rows), published: true }
    }

    /// No such table in the index, which is not the same as an empty one.
    pub fn absent() -> Boosts {
        Boosts::default()
    }

    /// The table as `source` publishes it, [`Self::absent`] where it
    /// publishes none.
    pub async fn read(source: &dyn Source) -> io::Result<Boosts> {
        Ok(table::<BoostTable>(source)
            .await?
            .map_or_else(Boosts::absent, Boosts::of))
    }

    /// Whether the index published a supercharge table at all
    ///
    /// False is "the map cannot say where a jet cone is", not "there are
    /// none". Rebuilt with
    /// `galos ingest --from database --index DIR --only boosts`.
    pub fn published(&self) -> bool {
        self.published
    }

    /// The rows, for the coarse graph [`crate::highway`] sorts
    /// them into.
    pub fn table(&self) -> &[SystemBoost] {
        &self.rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a boost is worth is the boost's, told which drive takes it
    #[test]
    fn a_boost_says_what_it_multiplies_a_jump_by() {
        assert_eq!(Boost::Neutron.factor(Fsd::MkI), 4.);
        assert_eq!(Boost::WhiteDwarf.factor(Fsd::MkI), 1.5);
        assert_eq!(Boost::Neutron.factor(Fsd::MkII), 6.);
        assert_eq!(Boost::WhiteDwarf.factor(Fsd::MkII), 3.);
        for fsd in [Fsd::MkI, Fsd::MkII] {
            assert_eq!(Boost::widest(fsd), Boost::Neutron.factor(fsd));
        }
    }

    /// A class supercharges off the one reading of it the index takes
    #[test]
    fn a_class_says_what_it_can_supercharge() {
        let boost = |class| Boost::of(StarKind::of(class));
        assert_eq!(boost("N"), Some(Boost::Neutron));
        for dwarf in ["D", "DA", "DAB", "DBV", "DC", "DQ", "DX"] {
            assert_eq!(boost(dwarf), Some(Boost::WhiteDwarf), "{dwarf}");
        }
        for none in ["", "G", "K_OrangeGiant", "H", "MS", "TTS", "Neutron"] {
            assert_eq!(boost(none), None, "{none}");
        }
    }

    /// A table published before a row carried a place is brought forward
    /// at the place the payloads give, by an upgrade of the directory
    ///
    /// The map's perf guard found it: `invalid length 2, expected struct
    /// SystemBoost with 3 elements` over a directory `galos index migrate`
    /// had just said it had finished with. Asked a second time, there is
    /// nothing left to place.
    #[test]
    fn an_upgrade_places_a_table_written_without_places() {
        #[derive(Serialize)]
        struct Unplaced {
            address: i64,
            boost: Boost,
        }

        let dir = crate::testing::Scratch::new("unplaced");
        crate::testing::sky(dir.path(), &[(7, [1.0, 2.0, 3.0])]);
        galos_index::format::msgpack::write_meta(
            &galos_index::store::tables::path(dir.path(), BoostTable::NAME),
            &vec![Unplaced { address: 7, boost: Boost::Neutron }],
        )
        .expect("a table of the old shape");

        let tables = galos_index::TableSet::new().with::<BoostTable>();
        let rewrite = || {
            galos_index::ops::upgrade::rewrite(
                dir.path(),
                &tables,
                &|| false,
                &mut |_| {},
            )
            .expect("a rewrite")
        };
        assert_eq!(rewrite().upgraded, 1, "the table stayed behind");
        assert_eq!(
            galos_index::store::tables::read::<BoostTable>(dir.path())
                .expect("the table reads as placed rows"),
            Some(vec![SystemBoost {
                address: 7,
                boost: Boost::Neutron,
                position: [1.0, 2.0, 3.0],
            }]),
        );
        assert_eq!(rewrite().upgraded, 0, "a placed table was rewritten");
    }
}
