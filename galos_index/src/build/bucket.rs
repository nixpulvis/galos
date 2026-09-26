//! Spilling on arrival, and the regions formed from what arrived.
//!
//! A bucket is a fixed coarse cell of the cube, so a system's bucket falls
//! out of its position alone and nothing has to be counted before the
//! galaxy is read. Every system is appended to its bucket's spill as it is
//! pushed and the counts fall out of the writing, so the source is read
//! once.
//!
//! Regions are formed afterwards, from the buckets. The counts are the
//! grid [`Cut::of`] divides, which groups the sparse buckets into coarser
//! regions: a bucket is not a region on its own, since a bucket holding a
//! handful would be a region whose ancestors hold a handful, and that is
//! the one thing a [`Cut`] may not be.
//!
//! A bucket over the budget is divided the other way. Its spill is re-read
//! and re-bucketed one level deeper, and again, until every piece is
//! within budget or is a cell nothing divides — at [`MAX_LEVEL`], or
//! holding systems that share a position. Such a piece is a region over
//! budget; [`crate::build::cold`] counts it in its report rather than hiding it.
//!
//! One level deeper and never several: a piece is a region, and a region's
//! ancestors must each hold more than a leaf's worth. Every level stepped
//! over would be an ancestor no count had ever looked at.
//!
//! A record is therefore rewritten at most `MAX_LEVEL - BUCKET_LEVEL`
//! times, seventeen, and only where all but coincident systems keep a cell
//! over budget that far down. A galaxy whose buckets fit is written once
//! and read twice, for the offer and for the build.

use crate::build::region::Cut;
use crate::build::snapshot::BuildParams;
use crate::core::geometry::CellId;
use crate::core::geometry::MAX_LEVEL;
use crate::core::record::System;
use crate::format::layout::spill_path;
use crate::format::spill::{Spill, Spilled, as_bytes};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The level a bucket is a cell of.
///
/// `8^4` is 4,096 cells over the cube, of which the galaxy's disc fills a
/// few hundred: few enough to hold a buffer for each, fine enough that a
/// cut formed from them lands near the budget with little splitting.
pub const BUCKET_LEVEL: u8 = 4;

/// Records a bucket holds before it writes them.
///
/// A bucket's file is opened, appended to and closed on each flush, so one
/// handle is open at a time however many buckets there are. What is held
/// instead is 7 KiB a bucket that has filled once — 28 MiB if all 4,096
/// ever do, and a tenth of that over the disc.
const BUFFERED: usize = 128;

/// A galaxy being spilled, a system at a time, into its bucket's file.
pub struct Buckets {
    dir: PathBuf,
    held: HashMap<CellId, Bucket>,
}

/// One bucket: where it writes, what it has not written yet, how many it
/// has taken.
struct Bucket {
    path: PathBuf,
    buffer: Vec<System>,
    count: u64,
}

impl Bucket {
    /// Append what is buffered, holding the file only for the write.
    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let mut out =
            OpenOptions::new().create(true).append(true).open(&self.path)?;
        out.write_all(as_bytes(&self.buffer))?;
        self.buffer.clear();
        Ok(())
    }
}

impl Buckets {
    /// Spill into `dir`, which is emptied first.
    pub fn create(dir: &Path) -> io::Result<Buckets> {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir)?;
        Ok(Buckets { dir: dir.to_owned(), held: HashMap::new() })
    }

    /// One more system, into the bucket its position falls in.
    pub fn push(&mut self, system: System) -> io::Result<()> {
        let id = CellId::of_point(system.position, BUCKET_LEVEL);
        let dir = &self.dir;
        let bucket = self.held.entry(id).or_insert_with(|| Bucket {
            path: spill_path(dir, id),
            buffer: Vec::new(),
            count: 0,
        });
        bucket.buffer.push(system);
        bucket.count += 1;
        if bucket.buffer.len() >= BUFFERED {
            bucket.flush()?;
        }
        Ok(())
    }

    /// How many systems have been pushed.
    pub fn count(&self) -> u64 {
        self.held.values().map(|bucket| bucket.count).sum()
    }

    /// Flush every bucket, answering where each landed and what it holds.
    pub fn finish(mut self) -> io::Result<HashMap<CellId, (PathBuf, u64)>> {
        let mut spilled = HashMap::with_capacity(self.held.len());
        for (&id, bucket) in self.held.iter_mut() {
            bucket.flush()?;
            spilled.insert(id, (bucket.path.clone(), bucket.count));
        }
        Ok(spilled)
    }
}

/// The regions the buckets came to, and where each one's systems are.
///
/// The bucket files are consumed making these: each is renamed in place,
/// concatenated into a coarser region, or divided into finer ones.
pub struct Formed {
    /// The cut, for the crown and the region builds under it.
    pub cut: Cut,
    /// Where each region's systems were left.
    pub spills: HashMap<CellId, PathBuf>,
}

/// Group `buckets` into regions no larger than `budget`, splitting any
/// bucket that is larger than that on its own.
///
/// The cut this answers meets [`Cut`]'s precondition. Every region is
/// either one [`Cut::of`] chose over the bucket counts, which only divides
/// a cell over the budget, or a piece of one, which is only ever taken
/// from a cell over the budget — and the budget is at least a leaf's
/// worth, so no region has an ancestor holding less.
pub fn form(
    dir: &Path,
    buckets: HashMap<CellId, (PathBuf, u64)>,
    budget: u64,
    params: &BuildParams,
) -> io::Result<Formed> {
    let budget = budget.max(params.leaf_cap as u64);
    let counted = buckets.iter().map(|(&id, &(_, count))| (id, count));
    let coarse = Cut::of(counted, budget, params);
    let held: HashSet<CellId> = coarse.regions().iter().copied().collect();

    let mut members: HashMap<CellId, Vec<CellId>> = HashMap::new();
    for &id in buckets.keys() {
        let mut at = id;
        while !held.contains(&at) {
            at = at.parent().expect("a bucket lies under some region");
        }
        members.entry(at).or_default().push(id);
    }
    let mut coarse: Vec<(CellId, Vec<CellId>)> = members.into_iter().collect();
    coarse.sort_by_key(|(region, _)| (region.level, region.morton()));

    let mut spills = HashMap::new();
    for (region, mut of_region) in coarse {
        of_region.sort_by_key(|bucket| bucket.morton());
        let count = of_region.iter().map(|bucket| buckets[bucket].1).sum();
        let path = gather(dir, region, &of_region, &buckets)?;
        divide(dir, region, path, count, budget, &mut spills)?;
    }
    Ok(Formed { cut: Cut::over(spills.keys().copied()), spills })
}

/// One file holding every system of `region`.
///
/// A region that is a whole bucket already has one and keeps it. Anything
/// coarser is the concatenation of its buckets, which is 56 bytes a system
/// copied once — scratch to scratch, against a source read that is not.
fn gather(
    dir: &Path,
    region: CellId,
    of_region: &[CellId],
    buckets: &HashMap<CellId, (PathBuf, u64)>,
) -> io::Result<PathBuf> {
    if of_region.len() == 1 && of_region[0] == region {
        return Ok(buckets[&region].0.clone());
    }
    let path = spill_path(dir, region);
    let mut out = File::create(&path)?;
    for bucket in of_region {
        let from = &buckets[bucket].0;
        io::copy(&mut File::open(from)?, &mut out)?;
        std::fs::remove_file(from)?;
    }
    out.flush()?;
    Ok(path)
}

/// Divide `region` until every piece is within `budget`, leaving each
/// piece's file in `spills`.
///
/// A piece over budget that no level divides is emitted as it stands: a
/// region too large is a build that needs more memory, and the count comes
/// out in [`crate::build::cold::ColdReport::over_budget`].
fn divide(
    dir: &Path,
    region: CellId,
    path: PathBuf,
    count: u64,
    budget: u64,
    spills: &mut HashMap<CellId, PathBuf>,
) -> io::Result<()> {
    let mut work = vec![(region, path, count)];
    while let Some((cell, path, count)) = work.pop() {
        if count <= budget || cell.level >= MAX_LEVEL {
            spills.insert(cell, path);
            continue;
        }
        match split(dir, cell, &path)? {
            None => {
                spills.insert(cell, path);
            }
            Some(pieces) => {
                std::fs::remove_file(&path)?;
                work.extend(pieces);
            }
        }
    }
    Ok(())
}

/// `cell`'s spill re-read and re-bucketed one level deeper.
///
/// [`None`] where its systems share a position, which no level divides and
/// which would otherwise be chased down to [`MAX_LEVEL`] a level at a
/// time. Eight files at once, and the parent's records are read off the
/// mapping rather than the heap.
fn split(
    dir: &Path,
    cell: CellId,
    path: &Path,
) -> io::Result<Option<Vec<(CellId, PathBuf, u64)>>> {
    let spilled = Spilled::open(path)?;
    let systems = spilled.systems();
    let Some(first) = systems.first() else {
        return Ok(Some(Vec::new()));
    };
    if systems.iter().all(|s| s.position == first.position) {
        return Ok(None);
    }

    let level = cell.level + 1;
    let mut pieces: HashMap<CellId, Spill> = HashMap::new();
    for &system in systems {
        let into = CellId::of_point(system.position, level);
        match pieces.entry(into) {
            Entry::Occupied(held) => held.into_mut(),
            Entry::Vacant(empty) => {
                empty.insert(Spill::create(&spill_path(dir, into))?)
            }
        }
        .push(system)?;
    }

    let mut divided = Vec::with_capacity(pieces.len());
    for (into, spill) in pieces {
        let count = spill.count();
        divided.push((into, spill.finish()?, count));
    }
    Ok(Some(divided))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::region::{Crown, Offer, joined};
    use crate::build::snapshot::Snapshot;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Somewhere to spill into, removed with the value.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let at = std::env::temp_dir().join(format!(
                "galos-bucket-{}-{}-{}",
                name,
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let _ = std::fs::remove_dir_all(&at);
            Scratch(at)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// A value in `-span/2 ..= span/2`, to a thousandth of the span.
        fn near(&mut self, span: f64) -> f64 {
            (self.next() % 1_001) as f64 / 1_000.0 * span - span / 2.0
        }
    }

    /// The middle of a cell, so a clump around it stays inside it.
    fn middle(cell: CellId) -> [f64; 3] {
        let min = cell.min_ly();
        let half = cell.edge_ly() / 2.0;
        [min[0] + half, min[1] + half, min[2] + half]
    }

    fn system(id: u64, position: [f64; 3], rng: &mut Rng) -> System {
        System {
            id64: id,
            position,
            absolute_magnitude: (rng.next() % 2_000) as f64 / 100.0 - 5.0,
            temperature: 3_000.0 + (rng.next() % 20_000) as f64,
            age_bucket: (rng.next() % 8) as u32,
            updated_at: 1_700_000_000 + (id as u32 % 1_000),
            kind: crate::core::record::StarKind::G,
        }
    }

    /// `n` systems scattered through a `span`-wide ball around `at`.
    fn clump(at: [f64; 3], span: f64, n: u64, rng: &mut Rng) -> Vec<System> {
        (0..n)
            .map(|i| {
                let position = [
                    at[0] + rng.near(span),
                    at[1] + rng.near(span),
                    at[2] + rng.near(span),
                ];
                system(i + 1, position, rng)
            })
            .collect()
    }

    /// Push `systems` and form the regions, as a build does.
    fn formed(
        dir: &Path,
        systems: &[System],
        budget: u64,
        params: &BuildParams,
    ) -> Formed {
        let mut buckets = Buckets::create(dir).expect("a scratch directory");
        for &s in systems {
            buckets.push(s).expect("a system spilled");
        }
        assert_eq!(buckets.count(), systems.len() as u64);
        let spilled = buckets.finish().expect("the buckets flushed");
        form(dir, spilled, budget, params).expect("regions formed")
    }

    /// A region's systems, read back off its spill.
    fn held(formed: &Formed, region: CellId) -> Vec<System> {
        Spilled::open(&formed.spills[&region])
            .expect("a region's spill")
            .systems()
            .to_vec()
    }

    /// The ancestor of `cell` at `level`.
    fn ancestor(cell: CellId, level: u8) -> CellId {
        let up = cell.level - level;
        CellId { level, x: cell.x >> up, y: cell.y >> up, z: cell.z >> up }
    }

    /// A bucket holding more than the budget is divided, and the pieces
    /// hold between them exactly what the bucket held.
    #[test]
    fn a_bucket_over_budget_is_split() {
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let at = Scratch::new("split");
        let mut rng = Rng(0x5EED);
        let core = CellId { level: BUCKET_LEVEL, x: 8, y: 8, z: 6 };
        let systems = clump(middle(core), 400.0, 5_000, &mut rng);

        let formed = formed(&at.0, &systems, 1_000, &params);
        assert!(
            formed.cut.regions().len() > 1,
            "one bucket of 5,000 under a budget of 1,000 stayed whole",
        );

        let mut total = 0;
        for &region in formed.cut.regions() {
            assert!(
                region.level > BUCKET_LEVEL
                    && ancestor(region, BUCKET_LEVEL) == core,
                "{region:?} is not a piece of the bucket that was split",
            );
            total += held(&formed, region).len();
        }
        assert_eq!(total, systems.len(), "the pieces lost or gained systems");
    }

    /// Buckets holding a handful are grouped rather than cut on: every
    /// region's ancestors hold more than a leaf's worth, which is what a
    /// [`Cut`] must promise and what a by-level cut cannot.
    ///
    /// The galaxy is one dense bucket and a scattering of lone systems far
    /// from it, so both roads through [`form`] are taken at once.
    #[test]
    fn a_cut_from_buckets_has_no_sparse_ancestor() {
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let at = Scratch::new("sparse");
        let mut rng = Rng(0xA11CE);
        let core = CellId { level: BUCKET_LEVEL, x: 8, y: 8, z: 6 };
        let mut systems = clump(middle(core), 400.0, 3_000, &mut rng);
        for (i, cell) in [(2u32, 9u32, 3u32), (13, 8, 12), (3, 7, 14)]
            .into_iter()
            .enumerate()
        {
            let lone =
                CellId { level: BUCKET_LEVEL, x: cell.0, y: cell.1, z: cell.2 };
            let id = 100_000 + i as u64;
            systems.push(system(id, middle(lone), &mut rng));
        }

        let budget = 500;
        let formed = formed(&at.0, &systems, budget, &params);
        let count = |cell: CellId| {
            systems
                .iter()
                .filter(|s| CellId::of_point(s.position, cell.level) == cell)
                .count()
        };

        // The sparse case is present: a bucket holding one system, which is
        // not a region, because a region there would be one a whole build
        // never raises.
        let lone = CellId { level: BUCKET_LEVEL, x: 2, y: 9, z: 3 };
        assert_eq!(count(lone), 1);
        assert!(
            !formed.cut.regions().contains(&lone),
            "a bucket holding one system was made a region",
        );

        for &region in formed.cut.regions() {
            let mut at = region;
            while let Some(up) = at.parent() {
                assert!(
                    count(up) > params.leaf_cap,
                    "{region:?} has an ancestor {up:?} holding {}, which is \
                     no more than a leaf's worth",
                    count(up),
                );
                at = up;
            }
        }

        // And the cut builds the galaxy a whole build would have built.
        let whole = Snapshot::build(&systems, &params);
        let offers: Vec<Offer> = formed
            .cut
            .regions()
            .iter()
            .map(|&r| Offer::of(r, held(&formed, r), &params))
            .collect();
        assert_eq!(
            offers.iter().map(Offer::count).sum::<u64>(),
            systems.len() as u64,
        );
        let crown = Crown::over(&offers, &params);

        let mut payloads = crown.built().payloads.clone();
        let mut indexes = Vec::new();
        for &region in formed.cut.regions() {
            let built = Snapshot::of_region(
                region,
                &held(&formed, region),
                crown.claimed(),
                &params,
            );
            payloads.extend(built.payloads.clone());
            indexes.push(built.index);
        }
        let index = joined(&crown, indexes.iter());

        assert_eq!(index.len(), whole.index.len(), "a different set of cells");
        for cell in whole.index.cells() {
            let built = index
                .get(cell.id)
                .unwrap_or_else(|| panic!("{:?} is missing", cell.id));
            assert_eq!(
                (built.rank_lo, built.rank_hi, built.child_mask),
                (cell.rank_lo, cell.rank_hi, cell.child_mask),
                "{:?} differs",
                cell.id,
            );
            assert_eq!(
                payloads.get(&cell.id).map_or(&[][..], Vec::as_slice),
                whole.payload(cell.id),
                "{:?} owns different systems",
                cell.id,
            );
        }
    }

    /// Systems sharing a position are one region however deep the split
    /// goes, so the split stops there rather than walking to
    /// [`MAX_LEVEL`] a level at a time.
    #[test]
    fn systems_on_one_point_are_one_region() {
        let params = BuildParams { internal_slice: 8, leaf_cap: 32 };
        let at = Scratch::new("point");
        let mut rng = Rng(0xDEAD);
        let core = CellId { level: BUCKET_LEVEL, x: 8, y: 8, z: 6 };
        let on = middle(core);
        let systems: Vec<System> =
            (1..=400).map(|id| system(id, on, &mut rng)).collect();

        let formed = formed(&at.0, &systems, 100, &params);
        assert_eq!(formed.cut.regions(), &[core]);
        assert_eq!(held(&formed, core).len(), systems.len());
    }
}
