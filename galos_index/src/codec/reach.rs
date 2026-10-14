//! Each cell's systems' reaches: how far each one reaches from its arrival
//! star, beside its payload.
//!
//! `reach/<shard>/<cell>.bin` beside each payload, shaped as a cell's light
//! is ([`crate::codec::lights`]): a header and one column, each system's
//! reach in metres **in the payload's own order**, so the `at`th of one is
//! the `at`th of the other and the two need no join. A system nothing has
//! been scanned in holds `NaN`, which no reach is.
//!
//! Why a file a cell and not a table: a reach is the one thing about a
//! system that really changes with the feed — a scan arrives and the system
//! grows — and the map only wants it for the systems near enough to draw
//! whole. As one table, every scan anywhere moved the stamp of 76 M rows
//! and every reader read all of them again; as a file a cell, a scan
//! rewrites a few kilobytes and a reader reads the cells near it.
//!
//! The builders keep the table by address ([`REACHES_FILE`]) and spread it
//! into the cells with [`Directory::spread_reaches`], which reads each
//! payload's addresses and writes the column. A cell none of whose systems
//! has a reach has no file, and a missing file reads as none on record.
//!
//! Every file is written beside its path and renamed over it, as a payload
//! is, so a reader holding one never sees it torn. A column of another
//! length than the payload it stands beside is one written for an older or
//! newer payload, and is read as none until the two agree again.
//!
//! [`REACHES_FILE`]: crate::codec::layout::REACHES_FILE

use crate::codec::Directory;
use crate::codec::bytes::{Decode, Encode};
use crate::codec::layout::{REACH_DIR, reach_path, reaches_path};
use crate::codec::tables::each_row;
use crate::core::geometry::CellId;
use crate::records::SystemReach;
use crate::tree::index::Index;
use std::fs;
use std::io;
use std::path::PathBuf;

/// The magic at the head of a cell's reaches.
const REACH_MAGIC: [u8; 4] = *b"GRCH";

/// The layout the files are written at.
const VERSION: u16 = 1;

/// A cell's reaches' header: magic, version, how many systems.
const REACH_HEADER: usize = 4 + 2 + 4;

/// A cell's systems' reaches as its file holds them: the header, then every
/// reach, `NaN` for none.
pub fn reach_bytes(reach: &[Option<f32>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(REACH_HEADER + reach.len() * 4);
    REACH_MAGIC.encode(&mut out);
    VERSION.encode(&mut out);
    (reach.len() as u32).encode(&mut out);
    for one in reach {
        one.unwrap_or(f32::NAN).encode(&mut out);
    }
    out
}

/// How many systems a cell's reaches are written for, or [`None`] for bytes
/// that are not one of these files.
pub fn reach_count(bytes: &[u8]) -> Option<usize> {
    let mut cur = bytes;
    if <[u8; 4]>::decode(&mut cur)? != REACH_MAGIC {
        return None;
    }
    if u16::decode(&mut cur)? != VERSION {
        return None;
    }
    Some(u32::decode(&mut cur)? as usize)
}

/// The first `limit` of a cell's systems' reaches, or [`None`] for bytes
/// that are not one of these files, or that are cut short of what they say.
pub fn reach_points(bytes: &[u8], limit: usize) -> Option<Vec<Option<f32>>> {
    let count = reach_count(bytes)?;
    if bytes.len() < REACH_HEADER + count * 4 {
        return None;
    }
    Some(
        (0..count.min(limit))
            .map(|at| {
                let from = REACH_HEADER + at * 4;
                let metres =
                    f32::from_le_bytes(bytes[from..from + 4].try_into().unwrap());
                (!metres.is_nan()).then_some(metres)
            })
            .collect(),
    )
}

/// The reaches table packed for lookup by address: two sorted columns.
///
/// What a spread reads every payload's addresses against. Twelve bytes a
/// row — about 0.9 GB over a galaxy's 76 M scanned systems — against the
/// map of rows [`crate::codec::tables::sidecars::Sidecars`] keeps.
#[derive(Clone, Debug, Default)]
pub struct Reaches {
    addresses: Box<[i64]>,
    metres: Box<[f32]>,
}

impl Reaches {
    /// Pack a table's rows. The last row for an address wins.
    pub fn of(mut rows: Vec<SystemReach>) -> Reaches {
        rows.reverse();
        rows.sort_by_key(|it| it.address);
        rows.dedup_by_key(|it| it.address);
        let addresses = rows.iter().map(|it| it.address).collect();
        let metres = rows.iter().map(|it| it.reach).collect();
        Reaches { addresses, metres }
    }

    /// The reaches table `dir` publishes, read a row at a time; empty where
    /// it publishes none.
    pub fn read(dir: &std::path::Path) -> io::Result<Reaches> {
        let mut rows = Vec::new();
        each_row(&reaches_path(dir), |row: SystemReach| {
            rows.push(row);
            Ok(())
        })?;
        Ok(Reaches::of(rows))
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

/// What a spread did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spread {
    /// Cells whose reaches were written.
    pub written: u64,
    /// Cells left with no file, none of their systems reaching anywhere.
    pub empty: u64,
}

impl Directory<'_> {
    /// Write one cell's systems' reaches, opening its shard directory the
    /// first time anything lands there, beside the file and renamed over it.
    pub fn write_reach(self, id: CellId, reach: &[Option<f32>]) -> io::Result<()> {
        let path = reach_path(self.root(), id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, reach_bytes(reach))?;
        fs::rename(&tmp, &path)
    }

    /// Remove one cell's systems' reaches, the cell owning nothing now or
    /// nothing scanned.
    pub fn remove_reach(self, id: CellId) -> io::Result<()> {
        match fs::remove_file(reach_path(self.root(), id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// The first `limit` of one cell's systems' reaches, in the payload's own
    /// order; empty where the cell has none on record.
    pub fn read_reach(
        self,
        id: CellId,
        limit: usize,
    ) -> io::Result<Vec<Option<f32>>> {
        match fs::read(reach_path(self.root(), id)) {
            Ok(bytes) => Ok(reach_points(&bytes, limit).unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// Where the per-cell reach files live, for a sweep to walk.
    pub(crate) fn reach_dir(self) -> PathBuf {
        self.root().join(REACH_DIR)
    }

    /// Write the reaches of `cells` off their payloads as they stand,
    /// looking each system up in `reach`
    ///
    /// The one way a reach gets into a cell: a build, a publish, a merge and
    /// a migration all write their payloads and then say which cells to
    /// spread into. A cell whose payload is gone, or none of whose systems
    /// reaches anywhere, is left with no file.
    pub fn spread_reaches(
        self,
        cells: impl IntoIterator<Item = CellId>,
        reach: &dyn Fn(i64) -> Option<f32>,
    ) -> io::Result<Spread> {
        let mut spread = Spread::default();
        for id in cells {
            let points = Index::read_payload(self.root(), id)?;
            let column: Vec<Option<f32>> =
                points.iter().map(|point| reach(point.id64 as i64)).collect();
            if column.iter().all(Option::is_none) {
                self.remove_reach(id)?;
                spread.empty += 1;
            } else {
                self.write_reach(id, &column)?;
                spread.written += 1;
            }
        }
        Ok(spread)
    }

    /// [`Self::spread_reaches`] over every cell the directory's index names,
    /// from the reaches table it publishes.
    ///
    /// What a cold build, a merge and a migration finish with. `stop` is
    /// asked between cells, and a spread cut short leaves every cell either
    /// as it was or written whole: running it again finishes it.
    pub fn spread_all_reaches(
        self,
        stop: &dyn Fn() -> bool,
    ) -> io::Result<Spread> {
        // No index is no cells: a directory not written yet, which has
        // nothing to spread into.
        let index = match Index::read(self.root()) {
            Ok(index) => index,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Spread::default());
            }
            Err(e) => return Err(e),
        };
        let reaches = Reaches::read(self.root())?;
        let mut spread = Spread::default();
        for cell in index.cells() {
            if stop() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    format!(
                        "{}: stopped spreading the reaches",
                        self.root().display()
                    ),
                ));
            }
            let one = self.spread_reaches([cell.id], &|a| reaches.get(a))?;
            spread.written += one.written;
            spread.empty += one.empty;
        }
        Ok(spread)
    }

    /// Whether the directory has any per-cell reaches, which a directory
    /// written before they existed has not.
    pub fn has_cell_reaches(self) -> bool {
        self.reach_dir().is_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::snapshot::{BuildParams, Snapshot};
    use crate::codec::cells::fixtures::{Scratch, systems};
    use crate::codec::layout::reach_path;
    use crate::codec::tables::Keyed;
    use crate::read::source::{FsSource, Part, Source};

    /// A reaches table holding every third system of `built`, each reaching
    /// as far as its address says, published in `dir`.
    fn every_third(dir: &std::path::Path, built: &Snapshot) -> Reaches {
        let mut table = Keyed::reaches();
        for cell in built.index.cells() {
            for point in built.payload(cell.id) {
                if point.id64 % 3 == 0 {
                    table.put(SystemReach {
                        address: point.id64 as i64,
                        reach: point.id64 as f32,
                    });
                }
            }
        }
        table.write(dir).expect("the reaches table");
        Reaches::read(dir).expect("the reaches table reads")
    }

    /// Every cell's reaches are its payload's systems' reaches, in its
    /// order; a cell with nothing scanned has no file; and a reader is
    /// handed the same column through a [`Source`].
    #[test]
    fn a_spread_writes_each_cells_reaches_in_its_payloads_order() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(9000), &BuildParams::default());
        built.write(&scratch.0).unwrap();
        let table = every_third(&scratch.0, &built);
        let dir = Directory::at(&scratch.0);
        assert!(!dir.has_cell_reaches());

        let spread = dir.spread_all_reaches(&|| false).unwrap();
        assert!(dir.has_cell_reaches());
        assert_eq!(
            (spread.written + spread.empty) as usize,
            built.index.cells().count(),
        );
        assert!(spread.written > 0, "nothing was spread");

        let source = FsSource::new(&scratch.0);
        for cell in built.index.cells() {
            let points = built.payload(cell.id);
            let want: Vec<Option<f32>> =
                points.iter().map(|it| table.get(it.id64 as i64)).collect();
            let read = pollster::block_on(source.reach(cell.id, usize::MAX))
                .unwrap();
            if want.iter().all(Option::is_none) {
                assert!(read.is_empty(), "{:?} has reaches", cell.id);
                assert!(!reach_path(&scratch.0, cell.id).exists());
            } else {
                assert_eq!(read, want, "{:?}", cell.id);
                let few = pollster::block_on(source.reach(cell.id, 2)).unwrap();
                assert_eq!(few, want[..want.len().min(2)]);
            }
        }
    }

    /// A reach moving alone rewrites its cell's reaches, and the stamp a
    /// reader asks about is theirs and not the payload's.
    #[test]
    fn a_reach_moving_alone_rewrites_its_cell() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(2000), &BuildParams::default());
        built.write(&scratch.0).unwrap();
        let cell = built.index.cells().find(|c| !built.payload(c.id).is_empty());
        let cell = cell.expect("a cell owning systems").id;
        let first = built.payload(cell)[0].id64 as i64;
        let dir = Directory::at(&scratch.0);
        let source = FsSource::new(&scratch.0);

        dir.spread_reaches([cell], &|_| None).unwrap();
        assert!(pollster::block_on(source.reach(cell, 9)).unwrap().is_empty());
        assert_eq!(pollster::block_on(source.stamp(Part::Reach(cell))).unwrap(), None);

        dir.spread_reaches([cell], &|a| (a == first).then_some(7.0)).unwrap();
        let read = pollster::block_on(source.reach(cell, 1)).unwrap();
        assert_eq!(read, [Some(7.0)]);
        assert!(pollster::block_on(source.stamp(Part::Reach(cell))).unwrap().is_some());
    }

    /// A column written for a payload of another length is read as none:
    /// what a reader landing between the payload and its reaches is told.
    #[test]
    fn a_column_of_another_length_reads_as_none() {
        let scratch = Scratch::new();
        let built = Snapshot::build(&systems(2000), &BuildParams::default());
        built.write(&scratch.0).unwrap();
        let cell = built.index.cells().find(|c| !built.payload(c.id).is_empty());
        let cell = cell.expect("a cell owning systems").id;
        let len = built.payload(cell).len();
        Directory::at(&scratch.0)
            .write_reach(cell, &vec![Some(1.0); len + 1])
            .unwrap();
        let source = FsSource::new(&scratch.0);
        assert!(pollster::block_on(source.reach(cell, 9)).unwrap().is_empty());
    }

    /// A cell's reaches round-trip, a system with none on record included,
    /// and bytes cut short or of another kind are refused.
    #[test]
    fn a_cells_reaches_round_trip() {
        let reach = [Some(1.5e9), None, Some(3.0e12)];
        let bytes = reach_bytes(&reach);
        assert_eq!(reach_count(&bytes), Some(3));
        assert_eq!(reach_points(&bytes, usize::MAX).unwrap(), reach);
        assert_eq!(reach_points(&bytes, 2).unwrap(), reach[..2]);
        assert!(reach_points(&bytes[..bytes.len() - 1], 9).is_none());
        assert!(reach_points(b"rubbish, and more", 9).is_none());
    }

    /// The last row for an address wins, and an address with none answers
    /// none.
    #[test]
    fn the_table_packs_by_address() {
        let reaches = Reaches::of(vec![
            SystemReach { address: 5, reach: 1.0 },
            SystemReach { address: 2, reach: 2.0 },
            SystemReach { address: 5, reach: 3.0 },
        ]);
        assert_eq!(reaches.len(), 2);
        assert_eq!(reaches.get(5), Some(3.0));
        assert_eq!(reaches.get(2), Some(2.0));
        assert_eq!(reaches.get(7), None);
    }
}
