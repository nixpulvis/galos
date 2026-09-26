//! Walking what the pack holds, a shard at a time: every address, or every
//! arrival star's class.

use super::Table;
use crate::format::layout::{
    BODIES_DIR, BODY_SHARDS, body_data_path, body_index_path,
};
use std::fs::File;
use std::io;
use std::path::Path;

/// Every system the pack holds bodies for, in address order.
/// What every packed system's arrival star is, shard by shard
///
/// **Sequential on purpose.** [`find`](super::find) maps a shard's index,
/// searches it and seeks the data file, which is right for one system and wrong
/// for ninety five million: a sweep that asked it per address would map the
/// same index a thousand times a shard and seek at random through gigabytes.
/// This maps each shard once and walks its live entries in the order they were
/// written.
///
/// `each` is handed the address and the class of the star a ship arrives at —
/// [`crate::records::derive::arrival_class`]'s answer, which is the rule the
/// boost table and the map's own panels read by. Systems with nothing scanned
/// are not offered at all.
///
/// Interruptible, a galaxy of scans being minutes of them, and what it
/// abandons costs nothing: the caller is filling in a column it can fill
/// again.
pub fn each_arrival_class(
    dir: &Path,
    stop: &dyn Fn() -> bool,
    each: &mut dyn FnMut(i64, &str),
) -> io::Result<u64> {
    let mut swept = 0u64;
    let bodies = dir.join(BODIES_DIR);
    let Ok(entries) = std::fs::read_dir(&bodies) else {
        return Ok(swept);
    };
    for shard in entries.flatten() {
        if stop() {
            return Ok(swept);
        }
        let path = shard.path();
        if path.extension().is_none_or(|it| it != "idx") {
            continue;
        }
        let table = Table::read(&path)?;
        let live = table.live();
        if live.is_empty() {
            continue;
        }
        let Some(shard) = path
            .file_stem()
            .and_then(|it| it.to_str())
            .and_then(|it| u64::from_str_radix(it, 16).ok())
        else {
            continue;
        };
        let data = body_data_path(dir, shard, table.generation);
        let Ok(file) = File::open(&data) else { continue };
        // SAFETY: a shard's data file is appended to and never rewritten in
        // place, and the mapping is dropped before the next shard.
        let mapped = unsafe { memmap2::Mmap::map(&file)? };

        // In the order the records were written rather than by address: a
        // sweep is a sequential read of the file and the addresses are
        // whatever order that gives.
        let mut rows: Vec<(i64, u64, u32)> = live
            .into_iter()
            .map(|(address, entry)| (address, entry.offset, entry.len))
            .collect();
        rows.sort_unstable_by_key(|&(_, offset, _)| offset);

        for (address, offset, len) in rows {
            if stop() {
                return Ok(swept);
            }
            let from = offset as usize + 4;
            let Some(bytes) = mapped.get(from..from + len as usize) else {
                continue;
            };
            let Ok(inside) =
                rmp_serde::from_slice::<crate::records::SystemBodies>(bytes)
            else {
                continue;
            };
            if let Some(class) = crate::records::derive::arrival_class(&inside)
            {
                each(address, class);
                swept += 1;
            }
        }
    }
    Ok(swept)
}

/// Every address the pack answers for, one shard's index at a time.
///
/// The cheap walk: the indexes are read and the data files are not
/// touched at all, which is the difference between this and
/// [`each_arrival_class`]. A caller weighing a galaxy's bodies against the
/// tree that names them wants exactly this and nothing decoded.
///
/// Interruptible, and what it abandons costs nothing: the caller is
/// counting, and a count cut short says so.
pub fn each_address(
    dir: &Path,
    stop: &dyn Fn() -> bool,
    each: &mut dyn FnMut(i64),
) -> io::Result<bool> {
    for shard in 0..BODY_SHARDS {
        if stop() {
            return Ok(false);
        }
        let path = body_index_path(dir, shard);
        if !path.exists() {
            continue;
        }
        for address in Table::read(&path)?.live().into_keys() {
            each(address);
        }
    }
    Ok(true)
}

/// Every address the pack answers for, in address order.
///
/// Held whole, which at a galaxy's scale is 76 million of them and 609 MB:
/// a caller that only wants to count them should take
/// [`each_address`] instead.
pub fn addresses(dir: &Path) -> io::Result<Vec<i64>> {
    let mut addresses = Vec::new();
    each_address(dir, &|| false, &mut |address| addresses.push(address))?;
    addresses.sort_unstable();
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{inside, scratch};
    use super::super::{remove, write};
    use super::*;
    use std::collections::HashMap;

    /// Every address, and only the ones still standing
    #[test]
    fn the_pack_says_which_systems_it_holds() {
        let dir = scratch("addresses");
        let addresses: Vec<i64> = (1..=32).map(|n| n * 99_991).collect();
        write(
            &dir,
            addresses
                .iter()
                .map(|&it| (it, inside(1)))
                .collect::<HashMap<_, _>>(),
        );
        remove(&dir, addresses[3]).expect("the withdrawal");

        let mut want: Vec<i64> = addresses
            .iter()
            .copied()
            .filter(|it| *it != addresses[3])
            .collect();
        want.sort_unstable();
        assert_eq!(addresses_of(&dir), want);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn addresses_of(dir: &Path) -> Vec<i64> {
        addresses(dir).expect("the pack lists")
    }
}
