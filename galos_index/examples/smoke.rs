//! Read a built index directory back through `FsSource`, the way the client
//! does, and report what came off disk. Proves the reader round-trips real
//! builder output, bodies with untagged enums included.
//!
//! `cargo run -p galos_index --example smoke -- <dir> [sample]`

use galos_index::Source;
use pollster::block_on;
use std::path::Path;

/// How many packed systems are read back where the caller names no number.
///
/// A spread of a few thousand, not the galaxy: what this asks of the bodies
/// is whether the record an index entry points at is the record that is
/// there, and a sample answers that as well as 76 million would in the
/// hours it would take.
const SAMPLE: usize = 2_000;

fn main() {
    block_on(run());
}

async fn run() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "galos_index".into());
    let sample: usize = std::env::args()
        .nth(2)
        .map(|it| it.parse().expect("a sample size"))
        .unwrap_or(SAMPLE);
    let source = galos_index::FsSource::new(&dir);

    let index = source.index().await.expect("index");
    let populated = source.populated().await.expect("populated");
    let names = source.names().await.expect("names");
    let factions = source.factions().await.expect("factions");
    println!(
        "index {} cells, {} populated, {} names, {} factions",
        index.len(),
        populated.len(),
        names.len(),
        factions.len(),
    );

    let mut read = Read::default();

    // The loose body files, which is how a directory built before the pack
    // keeps them: `bodies/<shard>/<address>.bin`, and loose in `bodies/`
    // for one older still. A published directory has none and the walk
    // finds nothing, which is why the pack is read below and not instead.
    let mut dirs = vec![Path::new(&dir).join("bodies")];
    while let Some(next) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let name = entry.file_name();
            let stem = name.to_string_lossy();
            let Some(address) = stem.strip_suffix(".bin") else { continue };
            let address: i64 = address.parse().expect("address filename");
            read.took(&source.bodies(address).await.expect("bodies decode"));
        }
    }
    println!("{read} off the loose files");

    // And the packed shards, which is where a published directory keeps
    // them. Every live address first — 8 bytes each, and the read of every
    // shard's index is a second — then a spread of them back through
    // `Source::bodies`, which is the road a click takes: the index
    // searched, the offset seeked, the MessagePack decoded. A compaction
    // that put an offset wrong is a decode failure here and nowhere else,
    // so a directory swept by `galos index sweep --bodies` is checked by
    // running this over it.
    let held = galos_index::pack::addresses(Path::new(&dir))
        .expect("the pack lists its systems");
    let step = (held.len() / sample.max(1)).max(1);
    let mut packed = Read::default();
    for &address in held.iter().step_by(step) {
        packed.took(&source.bodies(address).await.expect("bodies decode"));
    }
    println!(
        "{packed} off {} packed systems, every {step}th of {}",
        packed.files,
        held.len(),
    );
}

/// What the bodies read off a directory came to.
#[derive(Default)]
struct Read {
    files: usize,
    stars: usize,
    bodies: usize,
    surfaced: usize,
}

impl Read {
    /// One system's insides, decoded.
    fn took(&mut self, system: &galos_index::SystemBodies) {
        self.files += 1;
        self.stars += system.stars.len();
        self.bodies += system.bodies.len();
        self.surfaced +=
            system.bodies.iter().filter(|b| b.surface.is_some()).count();
    }
}

impl std::fmt::Display for Read {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} stars, {} bodies, {} with a surface",
            self.stars, self.bodies, self.surfaced,
        )
    }
}
