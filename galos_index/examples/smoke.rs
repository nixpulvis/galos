//! Read a built index directory back through `FsSource`, the way the client
//! does, and report what came off disk. Proves the reader round-trips real
//! builder output, bodies with untagged enums included.
//!
//! `cargo run -p galos_index --example smoke -- <dir>`

use galos_index::Source;
use pollster::block_on;
use std::path::Path;

fn main() {
    block_on(run());
}

async fn run() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "galos_index".into());
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

    // Read every body file back, which is where the untagged BodyType and
    // AtmosphereType enums have to decode. A single decode failure aborts.
    // Two levels, since the files are sharded: `bodies/<shard>/<address>.bin`
    // for a published directory, and loose in `bodies/` for one an older
    // builder wrote that has not been resharded yet.
    let bodies_dir = Path::new(&dir).join("bodies");
    let mut files = 0usize;
    let mut stars = 0usize;
    let mut bodies = 0usize;
    let mut surfaced = 0usize;
    let mut dirs = vec![bodies_dir];
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
            let system = source.bodies(address).await.expect("bodies decode");
            files += 1;
            stars += system.stars.len();
            bodies += system.bodies.len();
            surfaced +=
                system.bodies.iter().filter(|b| b.surface.is_some()).count();
        }
    }
    println!(
        "read {files} body files: {stars} stars, {bodies} bodies, \
         {surfaced} with a surface"
    );
}
