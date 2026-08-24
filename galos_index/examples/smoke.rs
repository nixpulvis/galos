//! Read a built index directory back through `FsSource`, the way the client
//! does, and report what came off disk. Proves the reader round-trips real
//! builder output, bodies with untagged enums included.
//!
//! `cargo run -p galos_index --example smoke -- <dir>`

use pollster::block_on;
use galos_index::Source;
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
    let bodies_dir = Path::new(&dir).join("bodies");
    let mut files = 0usize;
    let mut stars = 0usize;
    let mut bodies = 0usize;
    let mut surfaced = 0usize;
    if let Ok(entries) = std::fs::read_dir(&bodies_dir) {
        for entry in entries.flatten() {
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
