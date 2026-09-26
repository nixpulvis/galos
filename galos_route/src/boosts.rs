//! Where a drive can be supercharged: the published boost table, held.

use galos_index::Boost;
use std::sync::Arc;

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
/// The place comes with the row ([`galos_index::records::SystemBoost`]) and that is
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
    rows: Arc<Vec<galos_index::records::SystemBoost>>,
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
    pub fn of(rows: Vec<galos_index::records::SystemBoost>) -> Boosts {
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
    pub fn table(&self) -> &[galos_index::records::SystemBoost] {
        &self.rows
    }
}
