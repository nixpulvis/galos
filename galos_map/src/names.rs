//! The names table as the map holds it: packed, sorted, and without a
//! `String` or a hash bucket in sight.
//!
//! What the map used to hold was the table as `galos_index` hands it over —
//! `Vec<NameEntry>` and a `HashMap<i64, usize>` beside it — and measured
//! against `.index/full`'s 131,285,663 systems that is about **34 GB**:
//! 48 bytes of struct, a heap block for every name, and twenty-four more
//! bytes a system for the address index. On a 24 GiB machine the map was
//! swapping before it drew anything.
//!
//! The same table, packed:
//!
//! | | bytes a system | at 131.29 M |
//! |---|---|---|
//! | `addresses`, sorted | 8 | 1.05 GB |
//! | `positions` | 12 | 1.58 GB |
//! | `starts`, into the blob | 8 | 1.05 GB |
//! | the names themselves, once | ~25 | ~3.3 GB |
//! | | | **~7 GB** |
//!
//! Four things pay for that, and the first of them is the reason the rest
//! are possible:
//!
//! 1. **Nothing is allocated per system.** One array each, and one blob of
//!    name bytes. Where the table used to be 131 M heap blocks it is now
//!    four.
//! 2. **The addresses are sorted**, so an address is found by binary search
//!    over a contiguous gigabyte rather than by hashing into thirty. The
//!    `HashMap<i64, usize>` is gone, and with it three gigabytes.
//! 3. **Structure of arrays.** A lookup by address touches only
//!    `addresses`, so the search walks 8 bytes a step and not 30.
//! 4. **A name is bytes in a blob**, and every name is upper case
//!    ([`galos_index::SystemName`]), so a search compares bytes against
//!    bytes with no fold and no copy.
//!
//! This is the layout `TODO-map-scale.md` item 1 wants as a *file*: the
//! four arrays are exactly what a mapped part would hold, so the change
//! from this to mapping it is `Box<[T]>` becoming a slice of a mapping.
//! Nothing above this module would move.

use galos_index::meta::{NameEntry, SystemReach};
use galos_index::name::SystemName;

/// Every system's name and place, packed.
///
/// Built once at startup and never written to; a refresh puts what the feed
/// has named since in an overlay beside it. See the module header for the
/// layout and what it costs.
#[derive(Debug, Default)]
pub struct Table {
    /// Every address, ascending. What a lookup binary-searches.
    addresses: Box<[i64]>,
    /// Where each system sits, in light years, the table's own precision.
    positions: Box<[[f32; 3]]>,
    /// Where each name starts in `blob`, with the end of the last name at
    /// the back: `starts` is one longer than the table, so a name is
    /// `blob[starts[i]..starts[i + 1]]` with no length array beside it.
    starts: Box<[u64]>,
    /// Every name, once, in address order.
    blob: Box<[u8]>,
}

impl Table {
    /// How many systems the table names.
    pub fn len(&self) -> usize {
        self.addresses.len()
    }

    /// Whether it names none.
    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
    }

    /// Where `address` sits in the table, if it is in it.
    pub fn index_of(&self, address: i64) -> Option<usize> {
        self.addresses.binary_search(&address).ok()
    }

    /// The address of the `at`th system.
    pub fn address_at(&self, at: usize) -> i64 {
        self.addresses[at]
    }

    /// The name of the `at`th system, borrowed out of the blob.
    ///
    /// Upper case, the table being built from [`SystemName`]s.
    pub fn name_at(&self, at: usize) -> &str {
        let (from, to) =
            (self.starts[at] as usize, self.starts[at + 1] as usize);
        // Written from `str`s, so the bytes between two starts are one.
        std::str::from_utf8(&self.blob[from..to]).unwrap_or_default()
    }

    /// Where the `at`th system sits.
    pub fn position_at(&self, at: usize) -> [f32; 3] {
        self.positions[at]
    }

    /// The `at`th system as an entry, which costs the name a copy.
    ///
    /// For a caller that holds what it is given — a selection, a search
    /// result, a route's ends. Everything that only reads goes through
    /// [`name_at`](Self::name_at) instead.
    pub fn entry_at(&self, at: usize) -> NameEntry {
        NameEntry {
            address: self.addresses[at],
            name: SystemName::new(self.name_at(at)),
            position: self.positions[at],
        }
    }

    /// Every system's address and place, for the router to bucket.
    ///
    /// Widened to `f64` here rather than stored so, the table's precision
    /// being what the index publishes and the map's arithmetic being what
    /// wants the width.
    pub fn points(&self) -> impl Iterator<Item = (i64, [f64; 3])> + '_ {
        (0..self.len()).map(|at| {
            let p = self.positions[at];
            (self.addresses[at], [p[0] as f64, p[1] as f64, p[2] as f64])
        })
    }

    /// Which systems' names hold `needle`, at most `limit` of them.
    ///
    /// `needle` is expected upper case, as every name in the blob is. The
    /// scan is over the blob rather than over entries, so it allocates
    /// nothing until something is found, and stops at `limit` rather than
    /// collecting the galaxy and sorting it.
    pub fn matching(&self, needle: &str, limit: usize) -> Vec<usize> {
        let mut found = Vec::new();
        for at in 0..self.len() {
            if self.name_at(at).contains(needle) {
                found.push(at);
                if found.len() >= limit {
                    break;
                }
            }
        }
        found
    }

    /// Which system is named exactly `name`, if one is.
    pub fn named_exactly(&self, name: &str) -> Option<usize> {
        (0..self.len()).find(|&at| self.name_at(at) == name)
    }
}

/// Pack a table a chunk at a time.
///
/// The names table is published in chunks of 64 Ki entries and a galaxy's
/// worth of them is gigabytes, so the map reads one chunk, packs it, and
/// drops it. Holding the whole table to build this one is what the whole
/// exercise is about not doing.
#[derive(Debug, Default)]
pub struct Packing {
    /// Address, place, and where the name landed, before the sort.
    rows: Vec<(i64, [f32; 3], u64, u32)>,
    blob: Vec<u8>,
}

impl Packing {
    /// Room for `systems`, where a caller knows roughly how many there are.
    pub fn with_capacity(systems: usize) -> Packing {
        Packing {
            rows: Vec::with_capacity(systems),
            // A procedural name is about twenty-five bytes.
            blob: Vec::with_capacity(systems * 25),
        }
    }

    /// Take one entry.
    pub fn push(&mut self, entry: &NameEntry) {
        let at = self.blob.len() as u64;
        self.blob.extend_from_slice(entry.name.as_bytes());
        let len = (self.blob.len() as u64 - at) as u32;
        self.rows.push((entry.address, entry.position, at, len));
    }

    /// Take every entry of a chunk, and let the chunk go.
    pub fn extend(&mut self, chunk: Vec<NameEntry>) {
        for entry in &chunk {
            self.push(entry);
        }
    }

    /// How many entries have been taken.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Sort by address and pack, which is the table.
    ///
    /// The names are copied into a second blob in address order: a search
    /// walks the blob, and a blob in address order is a blob a scan reads
    /// forwards.
    pub fn build(mut self) -> Table {
        self.rows.sort_by_key(|(address, ..)| *address);
        // The later of two rows under one address wins, and `dedup_by` keeps
        // the *first* of a run — so the later is copied over the one being
        // kept before the earlier is dropped. The sort is stable, so it left
        // them in the order the chunks were read, which is the order the
        // table was written in: "later" is "corrected".
        self.rows.dedup_by(|later, kept| match later.0 == kept.0 {
            true => {
                *kept = *later;
                true
            }
            false => false,
        });

        let mut addresses = Vec::with_capacity(self.rows.len());
        let mut positions = Vec::with_capacity(self.rows.len());
        let mut starts = Vec::with_capacity(self.rows.len() + 1);
        let mut blob = Vec::with_capacity(self.blob.len());
        for (address, position, at, len) in &self.rows {
            addresses.push(*address);
            positions.push(*position);
            starts.push(blob.len() as u64);
            let (from, to) = (*at as usize, *at as usize + *len as usize);
            blob.extend_from_slice(&self.blob[from..to]);
        }
        starts.push(blob.len() as u64);
        drop(self.blob);
        drop(self.rows);

        Table {
            addresses: addresses.into_boxed_slice(),
            positions: positions.into_boxed_slice(),
            starts: starts.into_boxed_slice(),
            blob: blob.into_boxed_slice(),
        }
    }
}

impl FromIterator<NameEntry> for Table {
    fn from_iter<I: IntoIterator<Item = NameEntry>>(entries: I) -> Table {
        let mut packing = Packing::default();
        for entry in entries {
            packing.push(&entry);
        }
        packing.build()
    }
}

/// How far each scanned system reaches, in metres, by address.
///
/// Sorted and packed for the same reasons the names are: a fifth of the
/// galaxy has a reach on record, which is 62 M rows, and a
/// `HashMap<i64, f32>` over them is about 1.5 GB against 744 MB here.
#[derive(Debug, Default)]
pub struct Reaches {
    addresses: Box<[i64]>,
    metres: Box<[f32]>,
}

impl Reaches {
    /// Pack the published table.
    pub fn of(mut rows: Vec<SystemReach>) -> Reaches {
        rows.sort_by_key(|it| it.address);
        rows.dedup_by_key(|it| it.address);
        let mut addresses = Vec::with_capacity(rows.len());
        let mut metres = Vec::with_capacity(rows.len());
        for row in &rows {
            addresses.push(row.address);
            metres.push(row.reach);
        }
        Reaches {
            addresses: addresses.into_boxed_slice(),
            metres: metres.into_boxed_slice(),
        }
    }

    /// How far `address` reaches, where anything in it has been scanned.
    pub fn get(&self, address: i64) -> Option<f32> {
        self.addresses.binary_search(&address).ok().map(|at| self.metres[at])
    }

    /// How many systems have a reach on record.
    pub fn len(&self) -> usize {
        self.addresses.len()
    }

    /// Whether none do.
    pub fn is_empty(&self) -> bool {
        self.addresses.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(address: i64, name: &str, x: f32) -> NameEntry {
        NameEntry {
            address,
            name: SystemName::new(name),
            position: [x, 0., 0.],
        }
    }

    /// Everything put in reads back out, by address and by name
    ///
    /// The table is four arrays and a blob and the map asks it the same
    /// three questions it asked a `Vec` and a `HashMap`: what is at this
    /// address, what is this one called, and which names hold this text.
    #[test]
    fn a_packed_table_answers_what_was_packed_into_it() {
        let table: Table = vec![
            entry(30, "COL 285 SECTOR WU-E C12-3", 3.),
            entry(10, "SOL", 1.),
            entry(20, "SHINRARTA DEZHRA", 2.),
        ]
        .into_iter()
        .collect();

        assert_eq!(table.len(), 3);
        // Sorted, whatever order they arrived in: the binary search is the
        // whole point of the layout.
        assert_eq!(
            (0..3).map(|at| table.address_at(at)).collect::<Vec<_>>(),
            vec![10, 20, 30],
        );

        let at = table.index_of(20).expect("the address is in the table");
        assert_eq!(table.name_at(at), "SHINRARTA DEZHRA");
        assert_eq!(table.position_at(at), [2., 0., 0.]);
        assert_eq!(table.entry_at(at), entry(20, "SHINRARTA DEZHRA", 2.));
        assert_eq!(table.index_of(11), None, "an address nothing names");

        assert_eq!(table.named_exactly("SOL"), table.index_of(10));
        assert_eq!(table.named_exactly("SOLITUDE"), None);
        assert_eq!(
            table
                .matching("SECTOR", 25)
                .into_iter()
                .map(|at| table.address_at(at))
                .collect::<Vec<_>>(),
            vec![30],
        );
        assert_eq!(
            table.matching("S", 2).len(),
            2,
            "the cap is what stops a search collecting the galaxy",
        );
    }

    /// A system named twice is in the table once, under the later name
    ///
    /// The published chunks are read in order and a system corrected in a
    /// later chunk has an entry in both. Two rows under one address would
    /// leave the binary search answering either, and the router bucketing
    /// the system twice.
    #[test]
    fn a_system_named_twice_is_packed_once() {
        let table: Table =
            vec![entry(7, "OLD NAME", 1.), entry(7, "NEW NAME", 2.)]
                .into_iter()
                .collect();

        assert_eq!(table.len(), 1);
        let at = table.index_of(7).expect("the one row");
        assert_eq!(table.name_at(at), "NEW NAME");
        assert_eq!(table.position_at(at), [2., 0., 0.]);
    }

    /// The places come out as the router wants them, widened
    #[test]
    fn the_points_are_the_places_the_router_buckets() {
        let table: Table =
            vec![entry(1, "A", 1.5), entry(2, "B", -2.5)].into_iter().collect();

        assert_eq!(
            table.points().collect::<Vec<_>>(),
            vec![(1, [1.5, 0., 0.]), (2, [-2.5, 0., 0.])],
        );
    }

    /// A reach is found by address, and an absence is an absence
    ///
    /// Which is how the map tells "small" from "not on record": a system
    /// with nothing scanned has no row here and is drawn at the stand-in.
    #[test]
    fn a_reach_is_found_or_is_not_on_record() {
        let reaches = Reaches::of(vec![
            SystemReach { address: 9, reach: 400. },
            SystemReach { address: 3, reach: 100. },
        ]);

        assert_eq!(reaches.len(), 2);
        assert_eq!(reaches.get(3), Some(100.));
        assert_eq!(reaches.get(9), Some(400.));
        assert_eq!(reaches.get(5), None, "a system with nothing scanned");
    }
}
