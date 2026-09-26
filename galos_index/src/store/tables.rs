//! The tables written whole beside the cells, and the ones dependents
//! contribute.
//!
//! Every such table is one MessagePack array of rows in address order at
//! `<name>.bin`, a row a system at most. [`Keyed`] is one held open across a
//! run: rows by address, and whether anything has moved since it was last
//! written. The index's own — the populated systems and the reaches — are
//! [`Keyed`] tables [`crate::store::sidecars::Sidecars`] names; a dependent's
//! is a [`Table`] it hands over in a [`TableSet`], which the index holds,
//! writes, spills, carries through a merge and compares without knowing what
//! a row says.
//!
//! **An absent table is not an empty one.** No file says "this directory
//! cannot tell"; an empty array says "there are none". A reader keeps the
//! two apart ([`read`]), and a table resumed from nothing remembers it was
//! absent so a writer can choose to publish it ([`Keyed::claim`]).

use crate::format::msgpack::{read_meta, write_meta};
use crate::format::rows::{self, Sheet, Sorted};
use crate::records::{Arrival, Table};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where the table called `name` is written within a directory.
pub fn path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.bin"))
}

/// A contributed table as `dir` publishes it, or [`None`] where it has no
/// such file.
pub fn read<T: Table>(dir: &Path) -> io::Result<Option<Vec<T::Row>>> {
    optional(&path(dir, T::NAME))
}

/// A contributed table's rows out of the bytes of its file, for a reader
/// handed the bytes by a transport rather than a path.
pub fn decode<T: Table>(bytes: &[u8]) -> io::Result<Vec<T::Row>> {
    rmp_serde::from_slice(bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// A table of rows keyed by address, held as a directory holds it.
///
/// Keyed by address rather than kept as the sorted array that is written:
/// a pass patches a handful of systems, and the sort is a write's own step,
/// the order being the format's so that the same content is the same
/// bytes.
///
/// It remembers two things beside its rows. **Moved**: a row put or taken
/// since the last write, which a write clears — so a pass run in chunks
/// writes each table once at its end, and a later chunk that moved nothing
/// cannot unsay an earlier one. **Absent**: the directory it was resumed
/// from had no file for it, which a write also clears.
#[derive(Clone)]
pub struct Keyed<R> {
    name: &'static str,
    key: fn(&R) -> i64,
    rows: HashMap<i64, R>,
    moved: bool,
    absent: bool,
}

impl<R: fmt::Debug> fmt::Debug for Keyed<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyed")
            .field("name", &self.name)
            .field("rows", &self.rows.len())
            .field("moved", &self.moved)
            .field("absent", &self.absent)
            .finish()
    }
}

impl<R: Serialize + DeserializeOwned + PartialEq> Keyed<R> {
    /// Nothing held, and nothing published to be absent from.
    pub fn new(name: &'static str, key: fn(&R) -> i64) -> Keyed<R> {
        Keyed { name, key, rows: HashMap::new(), moved: false, absent: false }
    }

    /// The table as `dir` publishes it, absent where it has no file.
    ///
    /// A table that is there and will not decode is an error: read as
    /// empty, it would be republished from the handful of addresses one
    /// pass touches and the rows already published would be gone.
    pub fn resume(
        name: &'static str,
        key: fn(&R) -> i64,
        dir: &Path,
    ) -> io::Result<Keyed<R>> {
        let read: Option<Vec<R>> = optional(&path(dir, name))?;
        let absent = read.is_none();
        let rows = read
            .unwrap_or_default()
            .into_iter()
            .map(|row| (key(&row), row))
            .collect();
        Ok(Keyed { name, key, rows, moved: false, absent })
    }

    /// What the table is called.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The row for `address`, where the table holds one.
    pub fn get(&self, address: i64) -> Option<&R> {
        self.rows.get(&address)
    }

    /// Put a row in, answering whether that changed the table.
    ///
    /// A row that reads exactly as the one held changes nothing and moves
    /// nothing — the common case, a feed reporting the same systems over
    /// and over.
    pub fn put(&mut self, row: R) -> bool {
        let address = (self.key)(&row);
        if self.rows.get(&address) == Some(&row) {
            return false;
        }
        self.rows.insert(address, row);
        self.moved = true;
        true
    }

    /// Take a system's row out, answering whether there was one.
    pub fn remove(&mut self, address: i64) -> bool {
        let gone = self.rows.remove(&address).is_some();
        self.moved |= gone;
        gone
    }

    /// How many rows it holds.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Every row it holds, in no order.
    pub fn rows(&self) -> impl Iterator<Item = &R> {
        self.rows.values()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether a row has been put or taken since the last write.
    pub fn moved(&self) -> bool {
        self.moved
    }

    /// Have the next write publish this, moved or not.
    pub fn touch(&mut self) {
        self.moved = true;
    }

    /// Have the next write publish this if the directory had no file for
    /// it — empty, if that is what it comes to.
    ///
    /// A table nothing in a run happened to move is a table never written,
    /// and to a client that absence says "this index cannot tell", which is
    /// a different answer from "there are none".
    pub fn claim(&mut self) {
        self.moved |= self.absent;
    }

    /// Write it to `dir` in address order, answering how many rows.
    pub fn write(&mut self, dir: &Path) -> io::Result<usize> {
        let mut table: Vec<&R> = self.rows.values().collect();
        table.sort_unstable_by_key(|row| (self.key)(row));
        write_meta(&path(dir, self.name), &table)?;
        self.moved = false;
        self.absent = false;
        Ok(table.len())
    }

    /// Write it only where it moved, answering whether it was written.
    pub fn publish(&mut self, dir: &Path) -> io::Result<bool> {
        if !self.moved {
            return Ok(false);
        }
        self.write(dir)?;
        Ok(true)
    }

    /// Take the rows another directory publishes, where `take` says to,
    /// answering how many changed this.
    ///
    /// Row over row, as a merge of two directories weighs them; an address
    /// `take` refuses is left as this table has it.
    pub fn carry(
        &mut self,
        from: &Path,
        take: &dyn Fn(i64, bool) -> bool,
    ) -> io::Result<u64> {
        let mut changed = 0;
        for row in
            optional::<Vec<R>>(&path(from, self.name))?.unwrap_or_default()
        {
            let address = (self.key)(&row);
            if take(address, self.rows.contains_key(&address)) && self.put(row)
            {
                changed += 1;
            }
        }
        Ok(changed)
    }
}

/// The tables dependents contribute, which every writer of a directory is
/// handed and every one of them writes.
///
/// Built once by whatever assembles the program — `galos` names the
/// router's — and passed down; nothing below knows what is in it. Cheap to
/// clone.
#[derive(Clone, Default)]
pub struct TableSet {
    each: Vec<Arc<dyn Contribution>>,
}

impl fmt::Debug for TableSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

impl TableSet {
    /// None at all.
    pub fn new() -> TableSet {
        TableSet::default()
    }

    /// And `T`.
    ///
    /// # Panics
    ///
    /// Where the set already holds a table of `T`'s name, or `T` is named
    /// for one of the index's own: two tables would be written to one file.
    pub fn with<T: Table>(mut self) -> TableSet {
        const OWN: [&str; 3] = ["populated", "reaches", "factions"];
        assert!(
            !OWN.contains(&T::NAME) && !self.names().any(|it| it == T::NAME),
            "two tables called {:?}",
            T::NAME,
        );
        self.each.push(Arc::new(Of::<T>(PhantomData)));
        self
    }

    /// What each is called.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.each.iter().map(|it| it.name())
    }

    /// Each of them.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Contribution> + '_ {
        self.each.iter().map(|it| &**it)
    }

    /// The one called `name`.
    pub fn get(&self, name: &str) -> Option<&dyn Contribution> {
        self.iter().find(|it| it.name() == name)
    }

    /// Only those of them `keep` names.
    pub fn only(&self, keep: impl Fn(&str) -> bool) -> TableSet {
        let each = self.each.iter().filter(|it| keep(it.name())).cloned();
        TableSet { each: each.collect() }
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.each.is_empty()
    }
}

/// One contributed [`Table`], with what a row is taken out of the type.
///
/// What [`TableSet`] holds, so that a set of tables with rows of different
/// types can be walked. Nothing outside this module implements it.
pub trait Contribution: Send + Sync {
    /// [`Table::NAME`].
    fn name(&self) -> &'static str;

    /// [`Table::ABOUT`].
    fn about(&self) -> &'static str;

    /// The table held open, empty.
    fn held(&self) -> Box<dyn Held>;

    /// The table held open, as `dir` publishes it.
    fn resume(&self, dir: &Path) -> io::Result<Box<dyn Held>>;

    /// A file to push the rows of a build into, in `scratch`.
    fn spill(&self, scratch: &Path) -> io::Result<Box<dyn Spill>>;

    /// [`Table::upgrade`].
    fn upgrade(&self, dir: &Path) -> io::Result<Option<usize>>;

    /// The table in two directories, row against row.
    fn compare(&self, a: &Path, b: &Path) -> io::Result<Agreement>;
}

/// One contributed table held open: a [`Keyed`] table whose rows are
/// derived from an [`Arrival`].
#[allow(clippy::len_without_is_empty)]
pub trait Held: Send + Sync + fmt::Debug {
    /// What the table is called.
    fn name(&self) -> &'static str;

    /// Take what the table derives from `arrival`, answering whether that
    /// changed it: a row put, or a row that stood taken out where the table
    /// has nothing to say about the system any more.
    fn arrive(&mut self, arrival: &Arrival) -> bool;

    /// Take a system's row out, answering whether there was one.
    fn remove(&mut self, address: i64) -> bool;

    /// Whether it holds a row for `address`.
    fn holds(&self, address: i64) -> bool;

    /// How many rows it holds.
    fn len(&self) -> usize;

    /// [`Keyed::moved`].
    fn moved(&self) -> bool;

    /// [`Keyed::touch`].
    fn touch(&mut self);

    /// [`Keyed::claim`].
    fn claim(&mut self);

    /// [`Keyed::write`].
    fn write(&mut self, dir: &Path) -> io::Result<usize>;

    /// [`Keyed::publish`].
    fn publish(&mut self, dir: &Path) -> io::Result<bool>;

    /// [`Keyed::carry`].
    fn carry(
        &mut self,
        from: &Path,
        take: &dyn Fn(i64, bool) -> bool,
    ) -> io::Result<u64>;
}

/// One contributed table's rows as a build derives them, pushed to a file
/// rather than held; see [`crate::store::sidecars::TableWriter`].
pub trait Spill: Send {
    /// What the table is called.
    fn name(&self) -> &'static str;

    /// Push what the table derives from `arrival`, if anything.
    fn arrive(&mut self, arrival: &Arrival) -> io::Result<()>;

    /// Push the rows `served` already publishes, ahead of any derived.
    fn seed(&mut self, served: &Path) -> io::Result<()>;

    /// Everything pushed, on disk.
    fn flush(&mut self) -> io::Result<()>;

    /// Sort the rows into the table in `dir`, answering how many.
    fn finish(
        self: Box<Self>,
        scratch: &Path,
        dir: &Path,
        budget: usize,
    ) -> io::Result<usize>;
}

/// A [`Table`] behind [`Contribution`]: nothing but its type.
struct Of<T>(PhantomData<fn() -> T>);

impl<T: Table> Contribution for Of<T> {
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn about(&self) -> &'static str {
        T::ABOUT
    }

    fn held(&self) -> Box<dyn Held> {
        Box::new(Derived::<T>(Keyed::new(T::NAME, T::address)))
    }

    fn resume(&self, dir: &Path) -> io::Result<Box<dyn Held>> {
        Ok(Box::new(Derived::<T>(Keyed::resume(T::NAME, T::address, dir)?)))
    }

    fn spill(&self, scratch: &Path) -> io::Result<Box<dyn Spill>> {
        let sheet = Sheet::open(scratch.join(format!("{}.rows", T::NAME)))?;
        Ok(Box::new(Spilled::<T> { sheet, marker: PhantomData }))
    }

    fn upgrade(&self, dir: &Path) -> io::Result<Option<usize>> {
        T::upgrade(dir)
    }

    fn compare(&self, a: &Path, b: &Path) -> io::Result<Agreement> {
        Ok(agreement(read::<T>(a)?, read::<T>(b)?, T::address))
    }
}

/// [`Held`] over a [`Keyed`] table of `T`'s rows.
struct Derived<T: Table>(Keyed<T::Row>);

impl<T: Table> fmt::Debug for Derived<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Table> Held for Derived<T> {
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn arrive(&mut self, arrival: &Arrival) -> bool {
        match T::derive(arrival) {
            Some(row) => self.0.put(row),
            None => self.0.remove(arrival.address),
        }
    }

    fn remove(&mut self, address: i64) -> bool {
        self.0.remove(address)
    }

    fn holds(&self, address: i64) -> bool {
        self.0.get(address).is_some()
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn moved(&self) -> bool {
        self.0.moved()
    }

    fn touch(&mut self) {
        self.0.touch()
    }

    fn claim(&mut self) {
        self.0.claim()
    }

    fn write(&mut self, dir: &Path) -> io::Result<usize> {
        self.0.write(dir)
    }

    fn publish(&mut self, dir: &Path) -> io::Result<bool> {
        self.0.publish(dir)
    }

    fn carry(
        &mut self,
        from: &Path,
        take: &dyn Fn(i64, bool) -> bool,
    ) -> io::Result<u64> {
        self.0.carry(from, take)
    }
}

/// [`Spill`] of `T`'s rows.
struct Spilled<T> {
    sheet: Sheet,
    marker: PhantomData<fn() -> T>,
}

impl<T: Table> Spill for Spilled<T> {
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn arrive(&mut self, arrival: &Arrival) -> io::Result<()> {
        match T::derive(arrival) {
            Some(row) => self.sheet.push(&row),
            None => Ok(()),
        }
    }

    fn seed(&mut self, served: &Path) -> io::Result<()> {
        each_row(&path(served, T::NAME), |row: T::Row| self.sheet.push(&row))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sheet.flush()
    }

    fn finish(
        mut self: Box<Self>,
        scratch: &Path,
        dir: &Path,
        budget: usize,
    ) -> io::Result<usize> {
        self.sheet.flush()?;
        sort_table::<T::Row>(
            self.sheet.path(),
            scratch,
            T::NAME,
            &path(dir, T::NAME),
            T::address,
            budget,
        )
    }
}

/// Whether one table says the same thing in two directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Agreement {
    /// Neither holds one.
    AbsentBoth,
    /// Only the first holds one.
    OnlyA,
    /// Only the second holds one.
    OnlyB,
    /// Both do, and this is how their rows compare.
    Rows(RowDiff),
}

/// Two tables' rows walked against each other by address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowDiff {
    /// Rows in the first.
    pub a: usize,
    /// Rows in the second.
    pub b: usize,
    /// Systems only the first has a row for.
    pub only_a: usize,
    /// Systems only the second has a row for.
    pub only_b: usize,
    /// Systems both have a row for that is not the same row, ascending.
    pub differing: Vec<i64>,
}

impl RowDiff {
    /// Whether the two say the same thing.
    pub fn same(&self) -> bool {
        self.only_a == 0 && self.only_b == 0 && self.differing.is_empty()
    }
}

/// Compare two tables of rows keyed by address.
///
/// Two sorted runs walked together: a row on one side and not the other is
/// a missing system, and one on both that is not the same row is a system
/// the two derivations say different things about.
pub fn agreement<R: PartialEq>(
    a: Option<Vec<R>>,
    b: Option<Vec<R>>,
    key: impl Fn(&R) -> i64,
) -> Agreement {
    let (mut left, mut right) = match (a, b) {
        (None, None) => return Agreement::AbsentBoth,
        (Some(_), None) => return Agreement::OnlyA,
        (None, Some(_)) => return Agreement::OnlyB,
        (Some(left), Some(right)) => (left, right),
    };
    left.sort_by_key(&key);
    right.sort_by_key(&key);
    let mut diff =
        RowDiff { a: left.len(), b: right.len(), ..RowDiff::default() };
    let (mut i, mut j) = (0usize, 0usize);
    while i < left.len() && j < right.len() {
        let (x, y) = (key(&left[i]), key(&right[j]));
        match x.cmp(&y) {
            std::cmp::Ordering::Less => {
                diff.only_a += 1;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                diff.only_b += 1;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                if left[i] != right[j] {
                    diff.differing.push(x);
                }
                i += 1;
                j += 1;
            }
        }
    }
    diff.only_a += left.len() - i;
    diff.only_b += right.len() - j;
    Agreement::Rows(diff)
}

/// A published table, or [`None`] where the directory has no such file.
///
/// A sidecar an older build never wrote is an absence rather than a failure,
/// and everything beside it is still good.
pub(crate) fn optional<T: DeserializeOwned>(
    path: &Path,
) -> io::Result<Option<T>> {
    match read_meta(path) {
        Ok(table) => Ok(Some(table)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Write one table from its rows, in address order, without the table ever
/// being in memory.
///
/// An external sort: runs of `budget` bytes are read back, sorted and
/// written out, and the runs are then merged. What it stands in for is a
/// map of every row the read derived — 22.4 MiB over a seven-day slice and
/// some 6 GiB over the galaxy, which was the last thing on this road that
/// the whole sky had to fit in.
///
/// The last row an address has still wins, and that survives the split
/// into runs: a run is a stretch of the row file, so every row in one is
/// older than every row in the next, and a stable sort leaves the rows
/// inside a run in the order they were written.
pub(crate) fn sort_table<T: Serialize + DeserializeOwned>(
    rows: &Path,
    scratch: &Path,
    name: &str,
    table: &Path,
    key: impl Fn(&T) -> i64,
    budget: usize,
) -> io::Result<usize> {
    let sorted = rows::sorted::<T>(rows, scratch, name, &key, budget)?;
    write_table::<T>(table, &sorted)?;
    Ok(sorted.count())
}

/// Write a sorted run of rows as the MessagePack array a reader expects.
///
/// The bytes [`write_meta`] would write and by the same road — beside the
/// file and renamed over it — but streamed: the array's length is known
/// before its elements are, so nothing past one row is held.
fn write_table<T: Serialize + DeserializeOwned>(
    path: &Path,
    sorted: &Sorted,
) -> io::Result<()> {
    use serde::Serializer as _;
    use serde::ser::SerializeSeq;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut out =
        rmp_serde::Serializer::new(BufWriter::new(File::create(&tmp)?));
    let mut seq = out
        .serialize_seq(Some(sorted.count()))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(mut framed) = sorted.rows()? {
        while let Some((row, _)) = framed.next::<T>()? {
            seq.serialize_element(&row)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        }
    }
    seq.end().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    out.into_inner().flush()?;
    std::fs::rename(&tmp, path)
}

/// Read a published table back a row at a time.
///
/// A table is one MessagePack array, and `read_meta` decodes it into a
/// `Vec`: fine for a pass that patches tens of systems, and a galaxy's
/// worth of rows in memory for a run that only means to walk it once. This
/// hands each row over as it is decoded instead. An absent table is no
/// rows rather than a failure — a directory that has published no reaches
/// has nothing to seed a resumed read with.
pub(crate) fn each_row<T: DeserializeOwned>(
    path: &Path,
    take: impl FnMut(T) -> io::Result<()>,
) -> io::Result<()> {
    use serde::de::DeserializeSeed;

    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut failed = None;
    let mut de = rmp_serde::Deserializer::new(BufReader::new(file));
    let each = Each { take, failed: &mut failed, marker: PhantomData };
    each.deserialize(&mut de)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    match failed {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The seed [`each_row`] walks an array with.
///
/// A seed rather than a `Vec` because the point is not to have one. The
/// caller's error rides out in `failed`: serde's own error type is the
/// decoder's, and a row the caller could not write is not a row that
/// failed to decode.
struct Each<'a, T, F> {
    take: F,
    failed: &'a mut Option<io::Error>,
    marker: PhantomData<fn() -> T>,
}

impl<'de, T, F> serde::de::DeserializeSeed<'de> for Each<'_, T, F>
where
    T: DeserializeOwned,
    F: FnMut(T) -> io::Result<()>,
{
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        de: D,
    ) -> Result<(), D::Error> {
        de.deserialize_seq(self)
    }
}

impl<'de, T, F> serde::de::Visitor<'de> for Each<'_, T, F>
where
    T: DeserializeOwned,
    F: FnMut(T) -> io::Result<()>,
{
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a table of rows")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        mut self,
        mut seq: A,
    ) -> Result<(), A::Error> {
        // The array is read to its end even after a write has failed: what
        // is being read is a file the run still has to be able to say
        // something about, and half a decode is not a state serde defines.
        while let Some(row) = seq.next_element::<T>()? {
            if self.failed.is_none() {
                if let Err(err) = (self.take)(row) {
                    *self.failed = Some(err);
                }
            }
        }
        Ok(())
    }
}

/// A contributed table for this crate's own tests: a row for every system
/// that arrives at a neutron star, and nothing for any other.
#[cfg(test)]
pub(crate) mod testing {
    use crate::core::record::StarKind;
    use crate::records::{Arrival, Table};
    use serde::{Deserialize, Serialize};

    pub(crate) struct Cones;

    #[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
    pub(crate) struct Cone {
        pub(crate) address: i64,
        pub(crate) position: [f32; 3],
    }

    impl Table for Cones {
        const NAME: &'static str = "cones";
        const ABOUT: &'static str = "Where a ship arrives at a neutron star.";
        type Row = Cone;

        fn address(row: &Cone) -> i64 {
            row.address
        }

        fn derive(arrival: &Arrival) -> Option<Cone> {
            (arrival.kind == StarKind::Neutron).then_some(Cone {
                address: arrival.address,
                position: arrival.position,
            })
        }
    }

    /// The set of just that one.
    pub(crate) fn tables() -> super::TableSet {
        super::TableSet::new().with::<Cones>()
    }

    /// A system arriving at a star of `kind`, at the origin.
    pub(crate) fn arriving(address: i64, kind: StarKind) -> Arrival {
        Arrival { address, kind, position: [0.0; 3] }
    }
}
