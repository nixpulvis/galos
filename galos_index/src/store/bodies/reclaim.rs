//! Folding a shard's tail into its base, and giving its dead records back:
//! a hole punched where the dead runs are whole blocks, a copy into the next
//! generation where they are not.
//!
//! What the write path does at [`Dead::Half`] and a sweep at
//! [`Dead::Worth`]; the weighing both of them decide by is [`Cost`].

use super::{ENTRY, Entry, HEADER, Table, header_bytes};
use crate::format::layout::{body_data_path, body_index_path};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// What a sweep of the shards gave back.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// Shards that gave anything back.
    pub shards: usize,
    /// Bytes the disk no longer holds.
    pub bytes: u64,
    /// Of those, the bytes punched out in place. The rest were copied —
    /// see `How`, which is where the two are chosen between.
    pub punched: u64,
    /// Whether every shard was looked at. A stop part way answers `false`,
    /// as does a shard that would not reclaim.
    pub finished: bool,
}

/// How much of a data file has to be dead before a fold rewrites it.
///
/// Dead is what the index points at none of: a record a later write
/// replaced, or one a tombstone withdrew. Reclaiming it is the shard's
/// live bytes written out again, so the bar is what that write is worth.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Dead {
    /// Half the file. What the write path keeps: a feed appending to a
    /// shard it has appended to for months pays the rewrite once the file
    /// holds twice the bytes it needs, and no sooner.
    Half,
    /// A tenth of the file, and at least `WORTH` of it. What a sweep
    /// asks by — an operator, or a build that has just rewritten the
    /// galaxy, is paying for the space rather than for the next append.
    Worth,
}

impl Dead {
    /// Whether what [`cost`] measured has reached this bar.
    pub(super) fn reached(self, cost: &Cost) -> bool {
        match self {
            // `>=`, and not `>`: a re-import replaces every record with
            // one of very nearly the same length, which leaves a shard
            // *at* half rather than past it. A strict test would refuse a
            // re-imported galaxy, the one case the rule is for.
            Dead::Half => cost.dead >= cost.live,
            // A tenth of what the file holds is `dead * 10 >= dead +
            // live`, which is this.
            Dead::Worth => cost.dead >= WORTH && cost.dead * 9 >= cost.live,
        }
    }
}

/// The least dead bytes worth rewriting a shard for.
///
/// A mebibyte, which is about 400 records. Below that the rewrite costs
/// more in writes than the file gives back in blocks.
const WORTH: u64 = 1 << 20;

/// Merge a shard's tail into its base, and reclaim the data file where
/// enough of it is dead.
///
/// The index is written beside and renamed over, so a reader sees one whole
/// index or the other. A compaction writes the *next* generation's data file
/// and leaves the old one until the index naming the new one is in place: a
/// reader holding offsets into the old bytes has to go on being right about
/// them until it reads the index again.
///
/// At [`Dead::Half`], which is the write path's bar. [`reclaim`] is the
/// same fold at a sweep's.
pub fn fold(dir: &Path, shard: u64) -> io::Result<()> {
    folded(dir, shard, Dead::Half).map(|_| ())
}

/// Give one shard's dead bytes back, where a sweep's bar is reached.
///
/// The same report a whole sweep answers, for one shard: `shards` is one
/// where it gave anything back and nought where it did not.
pub fn reclaim(dir: &Path, shard: u64) -> io::Result<Reclaimed> {
    folded(dir, shard, Dead::Worth)
}

/// One fold, at whichever bar the caller keeps.
fn folded(dir: &Path, shard: u64, bar: Dead) -> io::Result<Reclaimed> {
    let path = body_index_path(dir, shard);
    let table = Table::read(&path)?;
    let live = table.live();
    let data = body_data_path(dir, shard, table.generation);
    let cost = cost(&data, &live)?;

    // Half the file being records nothing points at is the write path's
    // trigger: a feed's thirty systems a second leave about six gigabytes
    // of dead records a day over the galaxy, which reaches half a shard in
    // a couple of months.
    let reclaiming = bar.reached(&cost);

    // Nothing to merge and nothing to reclaim.
    //
    // **An empty tail is not reason enough to stop here**: a folded shard
    // whose data file is half dead is exactly what a re-import leaves, and
    // no later write folds it because the tail it left is under
    // [`tail_bound`].
    if table.tail.is_empty() && !reclaiming {
        return Ok(Reclaimed { finished: true, ..Reclaimed::default() });
    }

    // Punched where the dead bytes are whole blocks and the file has not
    // outgrown what it holds, copied where they are not. See [`How`].
    let mut how = match reclaiming {
        true => How::of(&cost),
        false => How::Nothing,
    };
    let mut gave = Reclaimed { finished: true, ..Reclaimed::default() };

    // **Before the index, and the copy after it.** The two are not the
    // same operation: a punch takes bytes nothing points at and leaves
    // every live offset where it was, so a reader mapping the index at
    // any point either side of it reads the same records. The copy moves
    // them, which is why it has to hand the index over first.
    //
    // A filesystem that will not punch says so here — no hole is opened
    // by a failed `fcntl` — and the shard takes the copy instead.
    if how == How::Punch {
        match punched(&data, &cost) {
            Ok(()) => {
                gave = Reclaimed {
                    shards: (cost.punchable > 0) as usize,
                    bytes: cost.punchable,
                    punched: cost.punchable,
                    finished: true,
                }
            }
            Err(_) => how = How::Copy,
        }
    }

    let generation = match how {
        How::Copy => table.generation.wrapping_add(1),
        How::Punch | How::Nothing => table.generation,
    };
    let mut base = Vec::with_capacity(live.len());
    match how {
        How::Copy => {
            let to = body_data_path(dir, shard, generation);
            let mut out = io::BufWriter::new(File::create(&to)?);
            let mut from = File::open(&data)?;
            let mut at = 0u64;
            for entry in live.values() {
                from.seek(SeekFrom::Start(entry.offset))?;
                let mut framed = vec![0u8; 4 + entry.len as usize];
                from.read_exact(&mut framed)?;
                out.write_all(&framed)?;
                base.push(Entry { offset: at, ..*entry });
                at += framed.len() as u64;
            }
            out.flush()?;
            gave = Reclaimed {
                shards: (cost.dead > 0) as usize,
                bytes: cost.dead,
                punched: 0,
                finished: true,
            };
        }
        // In place, so the entries are the entries: every live offset
        // still names the byte it named before.
        How::Punch | How::Nothing => base.extend(live.values().copied()),
    }

    let mut bytes = Vec::with_capacity(HEADER + base.len() * ENTRY);
    bytes.extend_from_slice(&header_bytes(generation, base.len()));
    for entry in &base {
        entry.onto(&mut bytes);
    }
    let tmp = path.with_extension("idx.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;

    if how == How::Copy {
        // Nothing reads this generation any more: the index naming it is
        // gone, and a reader that had already read it retries on the miss.
        let _ = std::fs::remove_file(&data);
    }
    Ok(gave)
}

/// Punch every whole block of a shard's data file that nothing points at.
///
/// All or nothing as far as the caller is concerned: a filesystem that
/// refuses the first hole has opened none of them, and one that refuses a
/// later one has opened holes only in runs that were already dead. Either
/// way the error sends the shard down the copy, which reclaims whatever
/// is left.
fn punched(data: &Path, cost: &Cost) -> io::Result<()> {
    let file = OpenOptions::new().write(true).open(data)?;
    for &(at, len) in &cost.holes {
        punch(&file, at, len)?;
    }
    Ok(())
}

/// How a shard gives its dead bytes back.
///
/// **The copy is the expensive one.** It reads every live record and
/// writes it into the next generation — 160 GB moved to free 161 GB,
/// measured over a re-imported galaxy — because a reader holding an offset
/// into the old bytes must not be handed new ones.
///
/// A hole moves nothing and hands nobody anything: the live records stay
/// at the offsets the index already names, and the blocks under the dead
/// runs go back to the filesystem. Which is the shape a re-import leaves —
/// the first import's records are one run ahead of the second's, so a
/// shard is a single punch of about half the file.
///
/// It is not always available and not always enough:
///
/// - A filesystem that cannot punch answers an error, and the copy is what
///   the sweep falls back to.
/// - Blocks are the granularity, so dead records finely interleaved with
///   live ones — what a feed leaves — free nothing. Under [`PUNCHED`] of
///   the dead bytes, the copy is the honest answer.
/// - A punched file keeps its length, and appends go on past it, so a
///   shard punched for ever is a file whose length grows without bound —
///   and a `cp` or an `rsync` that does not understand holes copies the
///   length rather than the blocks. [`BLOAT`] is where that stops: past
///   it the shard is copied, which sets the length back to what it holds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum How {
    /// Punch the dead runs; the file keeps its offsets and its length.
    Punch,
    /// Write the live records into the next generation.
    Copy,
    /// Fold the index and leave the data file alone.
    Nothing,
}

/// What share of a shard's dead bytes a punch has to reach to be worth
/// preferring to the copy, as the divisor of `(n-1)/n`.
///
/// Four, so three quarters: below that the file is left mostly dead and
/// the copy is the thing that actually reclaims it.
const PUNCHED: u64 = 4;

/// How far a data file's length may run past the bytes it holds before a
/// shard is copied rather than punched again.
///
/// Four, which is three re-imports of a galaxy before a shard is rewritten
/// once. The length is what a tool that does not understand holes copies.
const BLOAT: u64 = 4;

impl How {
    /// Which way this shard gives its dead bytes back.
    pub(super) fn of(cost: &Cost) -> How {
        match cost.punchable * PUNCHED >= cost.dead * (PUNCHED - 1)
            && cost.length <= cost.live.saturating_mul(BLOAT)
        {
            true => How::Punch,
            false => How::Copy,
        }
    }
}

/// What a shard's data file costs, weighed against the index over it.
///
/// **Measured against the file's own extents, not against its size.**
/// Neither number is the truth on its own: a punched file keeps the
/// length it grew to, and an appended file is over-allocated past its end
/// — 84.6 MB of blocks behind a 79.1 MB shard, measured on APFS. Weighing
/// by either makes a shard that has just been swept look worth sweeping
/// again, for ever. What is dead is the bytes that are *in* the file, are
/// backed by blocks, and have no entry pointing at them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Cost {
    /// Bytes of record the index points at.
    pub(super) live: u64,
    /// Bytes the file holds that it points at none of.
    pub(super) dead: u64,
    /// Of the dead, what punching [`Cost::holes`] would free: whole
    /// blocks, and only the ones still backed by data.
    pub(super) punchable: u64,
    /// The file's length, which after a punch is more than it costs.
    pub(super) length: u64,
    /// What the disk holds for it, which is what `du` reports.
    pub(super) allocated: u64,
    /// The block-aligned dead runs, ready to punch.
    pub(super) holes: Vec<(u64, u64)>,
}

/// Weigh a data file against the live entries of the index over it.
pub(super) fn cost(
    data: &Path,
    live: &BTreeMap<i64, Entry>,
) -> io::Result<Cost> {
    let Ok(meta) = std::fs::metadata(data) else {
        return Ok(Cost::default());
    };
    #[cfg(unix)]
    let (length, allocated, block) = {
        use std::os::unix::fs::MetadataExt;
        (meta.len(), meta.blocks() * 512, meta.blksize().max(512))
    };
    #[cfg(not(unix))]
    let (length, allocated, block) = (meta.len(), meta.len(), 4096);

    let mut cost = Cost { length, allocated, ..Cost::default() };
    if length == 0 {
        return Ok(cost);
    }
    cost.live = live.values().map(|it| 4 + it.len as u64).sum();

    let file = File::open(data)?;
    let data = extents(&file, length);
    for (from, to) in gaps(live, length) {
        cost.dead += backed(&data, from, to);
        // Pulled *in* to the blocks inside the run: a hole is whole
        // blocks or it is nothing, and a range that is not block-aligned
        // is refused outright — `EINVAL` from `F_PUNCHHOLE`, measured —
        // rather than rounded for you.
        let (from, to) = (from.div_ceil(block) * block, to / block * block);
        if to > from {
            let backed = backed(&data, from, to);
            if backed > 0 {
                cost.punchable += backed;
                cost.holes.push((from, to - from));
            }
        }
    }
    Ok(cost)
}

/// The runs of a data file no live entry covers, in file order.
fn gaps(live: &BTreeMap<i64, Entry>, length: u64) -> Vec<(u64, u64)> {
    let mut spans: Vec<(u64, u64)> = live
        .values()
        .map(|it| (it.offset, it.offset + 4 + it.len as u64))
        .collect();
    spans.sort_unstable();

    let mut at = 0u64;
    let mut runs = Vec::new();
    for (from, to) in spans {
        if from > at {
            runs.push((at, from));
        }
        at = at.max(to);
    }
    if length > at {
        runs.push((at, length));
    }
    runs
}

/// Where a file's bytes actually are.
///
/// `SEEK_DATA` and `SEEK_HOLE`, walked once: a file that has never been
/// punched answers one extent and a swept one answers a handful. This is
/// what keeps the weighing honest across a sweep — the runs a previous
/// sweep punched are holes, and a hole is not dead weight, it is nothing
/// at all.
///
/// A filesystem that does not answer these is taken at its length, as
/// though it had no holes.
fn extents(file: &File, length: u64) -> Vec<(u64, u64)> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "linux",
        target_os = "android"
    ))]
    {
        use std::os::fd::AsRawFd;
        let fd = file.as_raw_fd();
        // SAFETY: two seeks on a live descriptor, neither of which moves
        // anything this process reads through.
        let seek = |from: u64, whence: libc::c_int| -> Option<u64> {
            match unsafe { libc::lseek(fd, from as libc::off_t, whence) } {
                -1 => None,
                at => Some(at as u64),
            }
        };
        let mut found = Vec::new();
        let mut at = 0u64;
        while at < length {
            let Some(from) = seek(at, libc::SEEK_DATA) else { break };
            let to = seek(from, libc::SEEK_HOLE).unwrap_or(length).min(length);
            if to <= from {
                break;
            }
            found.push((from, to));
            at = to;
        }
        // An answer of nothing is a file that is all hole; an error on
        // the first seek is a filesystem that does not answer, and it is
        // told apart by whether anything was found before the break.
        if !found.is_empty() || seek(0, libc::SEEK_DATA).is_none() {
            return found;
        }
    }
    let _ = file;
    vec![(0, length)]
}

/// How much of `[from, to)` is backed by bytes rather than by hole.
fn backed(extents: &[(u64, u64)], from: u64, to: u64) -> u64 {
    extents
        .iter()
        .map(|&(at, end)| end.min(to).saturating_sub(at.max(from)))
        .sum()
}

/// Give the blocks under `[at, at + len)` back to the filesystem.
///
/// The file keeps its length and the range reads as zeroes. Offset and
/// length must both be block-aligned; [`punched`] is the only caller, and
/// the holes it is handed were aligned by [`cost`].
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn punch(file: &File, at: u64, len: u64) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    /// `fpunchhole_t`, whose two spare words must be zero.
    #[repr(C)]
    struct Punchhole {
        flags: u32,
        reserved: u32,
        offset: libc::off_t,
        length: libc::off_t,
    }
    let hole = Punchhole {
        flags: 0,
        reserved: 0,
        offset: at as libc::off_t,
        length: len as libc::off_t,
    };
    // SAFETY: `F_PUNCHHOLE` reads one `fpunchhole_t` through the pointer,
    // which is live for the call, as is the descriptor.
    match unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PUNCHHOLE, &hole) } {
        -1 => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

/// The same, where the hole is `fallocate`'s to punch.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn punch(file: &File, at: u64, len: u64) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: a syscall against a live descriptor; nothing is read through
    // a pointer.
    let done = unsafe {
        libc::fallocate(
            file.as_raw_fd(),
            libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
            at as libc::off_t,
            len as libc::off_t,
        )
    };
    match done {
        -1 => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

/// And where it is nobody's: the copy is the whole of the reclaim.
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
)))]
fn punch(_file: &File, _at: u64, _len: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this platform cannot punch a hole in a file",
    ))
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{held, inside, scratch};
    use super::super::{Found, remove, write};
    use super::*;
    use crate::format::layout::body_shard;
    use crate::records::SystemBodies;
    use std::collections::HashMap;

    /// A folded shard answers exactly what the tail answered
    ///
    /// The fold is where the pack rewrites what it serves from, so it is
    /// where a system can be lost: a tombstone dropped too early puts a
    /// withdrawn scan back, and an entry merged the wrong way round serves
    /// the older of two writes.
    #[test]
    fn folding_keeps_what_the_tail_said() {
        let dir = scratch("fold");
        let shard = 1u64;
        let addresses: Vec<i64> = (0..64)
            .map(|n| n)
            .filter(|n| body_shard(*n) == body_shard(0))
            .collect();
        // One shard's worth, written a few times each, so the fold has
        // duplicates to merge.
        let mut want: HashMap<i64, SystemBodies> = HashMap::new();
        for round in 1..=3i16 {
            for &address in &addresses {
                let inside = inside(round);
                write(&dir, HashMap::from([(address, inside.clone())]));
                want.insert(address, inside);
            }
        }
        let withdrawn = addresses[0];
        remove(&dir, withdrawn).expect("the withdrawal");
        want.remove(&withdrawn);

        let _ = shard;
        fold(&dir, body_shard(addresses[0])).expect("the fold");

        let table =
            Table::read(&body_index_path(&dir, body_shard(addresses[0])))
                .expect("the index reads");
        assert!(table.tail.is_empty(), "the fold left a tail behind");
        assert_eq!(table.base.len(), want.len(), "the base is the wrong size");

        for (&address, inside) in &want {
            assert_eq!(
                held(&dir, address),
                Found::Bodies(inside.clone()),
                "a folded system reads as something else",
            );
        }
        assert_eq!(
            held(&dir, withdrawn),
            Found::Absent,
            "a fold kept a tombstone's system",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A compaction moves the records and nothing else
    ///
    /// Rewriting the data file is the one operation that invalidates every
    /// offset a reader might be holding, which is why it writes the next
    /// generation rather than the same file. What must survive it is what
    /// the pack answers.
    #[test]
    fn compaction_rewrites_the_records_and_keeps_the_answers() {
        let dir = scratch("compact");
        let address = 12_345_678_i64;
        let shard = body_shard(address);

        // Written enough times over that most of the data file is records
        // nothing points at any more.
        for round in 1..=8i16 {
            write(&dir, HashMap::from([(address, inside(round))]));
        }
        let before = std::fs::metadata(body_data_path(&dir, shard, 0))
            .expect("a data file")
            .len();

        fold(&dir, shard).expect("the fold");

        assert!(
            !body_data_path(&dir, shard, 0).exists(),
            "the old generation was left behind",
        );
        let after = std::fs::metadata(body_data_path(&dir, shard, 1))
            .expect("the next generation")
            .len();
        assert!(
            after < before / 2,
            "the compaction kept the dead records: {after} of {before}",
        );
        assert_eq!(
            held(&dir, address),
            Found::Bodies(inside(8)),
            "the compaction lost the live record",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shard weighs by what is in it, not by how large the file is
    ///
    /// Two readings that would be wrong taken off the file's size, from
    /// either end of it. Exactly half dead is the case rather than a corner
    /// of it — a re-import replaces every record with one of very nearly
    /// the same length, so a galaxy imported twice sits *at* half, which a
    /// strict `>` would refuse. And a file is not its length or its blocks:
    /// a punched shard keeps a length it no longer costs, and an appended
    /// one is over-allocated past its end (84.6 MB of blocks behind a
    /// 79.1 MB shard, measured), either of which would make a swept shard
    /// look worth sweeping again for ever.
    #[test]
    fn a_shard_weighs_by_what_is_in_it() {
        let at = |live: u64, dead: u64| Cost {
            live,
            dead,
            // The length and the blocks disagree with both, and with
            // each other, and neither is asked.
            length: 1 << 40,
            allocated: 0,
            ..Cost::default()
        };
        assert!(Dead::Half.reached(&at(50, 50)));
        assert!(!Dead::Half.reached(&at(51, 49)));

        // Nothing dead is nothing to rewrite, at either bar.
        assert!(!Dead::Half.reached(&at(100, 0)));
        assert!(!Dead::Worth.reached(&at(100, 0)));

        // The sweep asks a tenth of what the file holds, and never fewer
        // than [`WORTH`] bytes of it however large that tenth's share.
        assert!(Dead::Worth.reached(&at(9 * WORTH, WORTH)));
        assert!(!Dead::Worth.reached(&at(9 * WORTH + 1, WORTH - 1)));
        assert!(!Dead::Worth.reached(&at(90, 10)));
    }

    /// A dead run gives up its whole blocks, and a swept file says so
    ///
    /// Two claims about the weighing, and the second is what keeps a
    /// sweep from running for ever. The alignment: `F_PUNCHHOLE` refuses
    /// a range that is not a multiple of the block size outright —
    /// `EINVAL`, measured — so a run is pulled *in* to the blocks inside
    /// it and a run with no whole block in it is not a hole at all. And
    /// the read-back: the bytes a punch gave away are gone from the next
    /// weighing, because what is dead is measured against where the
    /// file's data actually is and not against its length.
    #[test]
    fn a_hole_is_the_whole_blocks_of_a_dead_run() {
        let dir = scratch("holes");
        let data = dir.join("probe.dat");
        std::fs::write(&data, vec![7u8; 40_960]).expect("a data file");
        let at = |address: i64, offset: u64, len: u32| {
            (address, Entry { address, offset, len })
        };
        // Live at [0, 100) and [20_000, 20_100), so the dead runs are
        // [100, 20_000) and [20_100, 40_960).
        let live = BTreeMap::from([at(1, 0, 96), at(2, 20_000, 96)]);
        let weighed = cost(&data, &live).expect("a weighing");
        assert_eq!(weighed.live, 200);
        assert_eq!(weighed.dead, 19_900 + 20_860);
        assert_eq!(weighed.holes, vec![(4096, 12_288), (20_480, 20_480)]);
        assert_eq!(weighed.punchable, 12_288 + 20_480);

        if punched(&data, &weighed).is_err() {
            // A filesystem with no holes in it; the copy is its road and
            // the rest of this is about holes.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let after = cost(&data, &live).expect("a second weighing");
        assert_eq!(after.length, weighed.length, "a punch moved the end");
        assert_eq!(
            after.allocated,
            weighed.allocated - weighed.punchable,
            "the disk did not give the blocks back",
        );
        // What is left dead is the edges of the runs the blocks did not
        // cover — 3,996 and 3,616 bytes either side of the first hole,
        // 380 before the second — and there is nothing left to punch.
        assert_eq!(after.dead, 3_996 + 3_616 + 380);
        assert_eq!(after.punchable, 0);
        assert!(after.holes.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The copy is what reclaims a file a punch cannot
    ///
    /// Three shapes, and the road each is owed. A re-import is the
    /// first: one run of dead records ahead of the live ones, which is a
    /// single punch of half the file. A feed leaves the second: records
    /// smaller than a block, dead ones between live ones, where punching
    /// frees almost nothing and only the copy reclaims it. The third is
    /// the one that stops a punched shard's length growing for ever.
    #[test]
    fn the_copy_takes_what_a_punch_cannot() {
        let half = 4u64 << 20;
        let reimported = Cost {
            live: half,
            dead: half,
            punchable: half - 8192,
            length: half * 2,
            allocated: half * 2,
            holes: Vec::new(),
        };
        assert_eq!(How::of(&reimported), How::Punch);

        let fed = Cost { punchable: 128 << 10, ..reimported.clone() };
        assert_eq!(How::of(&fed), How::Copy);

        let bloated = Cost { length: half * (BLOAT + 1), ..reimported.clone() };
        assert_eq!(How::of(&bloated), How::Copy);
    }
}
