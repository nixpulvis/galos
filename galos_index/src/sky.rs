//! The galaxy's places, read where they lie.
//!
//! A router needs two things of a galaxy and neither is a name: what is
//! within a jump of here, and where one system sits. The cell tree and its
//! payloads already answer both — the payloads *are* the spatially ordered
//! record array, one file a cell, holding each system's address and its
//! position at full precision — so the answer is to read them, not to build
//! something else out of them.
//!
//! What the map used to do instead was copy every position onto the heap and
//! bucket it into a grid of its own: at 200,071,629 systems, 32 s of build
//! and about 13.7 GB, paid on the click that asked for a route, before a
//! single jump was considered. The grid was a second spatial index over a
//! galaxy that already had one.
//!
//! So this holds the index (a few tens of megabytes, resident, as it always
//! was) and maps payloads as a query reaches them. Nothing is derived,
//! nothing is built, and nothing is resident that a query has not touched —
//! which is also what makes it safe under the feed: there is no structure to
//! go stale when a cell is republished, and [`crate::store`] renames a
//! payload into place rather than rewriting it, so a mapping a route is
//! holding keeps reading the galaxy the route started on.

use crate::geometry::CellId;
use crate::store::Payload;
use crate::walk::Index;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One system, as a search names it.
///
/// A cell and an offset into that cell's payload, which is the only name a
/// system has that costs nothing to arrive at: a neighbour query hands back
/// the cell it is scanning and the record it is on.
///
/// **Only meaningful within one [`Sky`].** A cell's payload is rewritten
/// whole when the feed moves a system into or out of it, so an offset is a
/// fact about the bytes a mapping holds and not about the galaxy. A search
/// keeps its `Sky` for as long as it runs and a node never outlives it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Node {
    pub cell: CellId,
    pub at: u32,
}

/// How many cells' payloads one [`Sky`] keeps mapped.
///
/// Mappings are cheap but not free: each is a kernel mapping, and a galaxy
/// is 204,466 cells, which is past what some systems allow one process
/// (`vm.max_map_count` is 65,530 by default on Linux). A route's working
/// set is a corridor and not a galaxy, so this needs to be large enough to
/// hold a corridor and no larger.
const MAPPED_CELLS: usize = 8 * 1024;

/// The galaxy as a router reads it: the resident cell tree, and the payloads
/// mapped as queries reach them.
pub struct Sky {
    dir: PathBuf,
    index: Index,
    held: Mutex<Held>,
}

/// The mapped payloads, in two generations.
///
/// A cap with a plain clear would throw a corridor away mid-route and read
/// it again. Two generations instead: new mappings land in `young`, and when
/// it fills, `young` becomes `old` and a fresh one starts. Anything a query
/// is still reaching for is found in `old` and promoted, so the set a route
/// actually uses survives, and the bound is twice [`MAPPED_CELLS`].
#[derive(Default)]
struct Held {
    young: HashMap<CellId, Option<Arc<Payload>>>,
    old: HashMap<CellId, Option<Arc<Payload>>>,
}

impl Sky {
    /// Read `dir`'s cell tree and stand ready to map its payloads.
    pub fn open(dir: &Path) -> io::Result<Sky> {
        Ok(Sky::of(dir, Index::read(dir)?))
    }

    /// The same over an index already read, which every client has.
    pub fn of(dir: &Path, index: Index) -> Sky {
        Sky {
            dir: dir.to_owned(),
            index,
            held: Mutex::new(Held::default()),
        }
    }

    /// The cell tree, for a caller that wants the aggregates.
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// How many systems the tree accounts for.
    pub fn len(&self) -> u64 {
        self.index.root().map_or(0, |root| root.aggregate.count())
    }

    /// Whether it accounts for none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One cell's payload, mapping it on the first ask.
    ///
    /// A cell that owns nothing is remembered as owning nothing, so a query
    /// sweeping the same empty neighbourhood does not stat its way through
    /// the galaxy twice.
    pub fn payload(&self, id: CellId) -> Option<Arc<Payload>> {
        let mut held = self.held.lock().expect("the mapped payloads");
        if let Some(found) = held.young.get(&id) {
            return found.clone();
        }
        if let Some(found) = held.old.remove(&id) {
            held.young.insert(id, found.clone());
            return found;
        }
        // A payload that will not map is treated as a cell that owns
        // nothing: a route over a directory being rebuilt under it should
        // come back short, not fail.
        let mapped = Payload::open(&self.dir, id).ok().flatten().map(Arc::new);
        if held.young.len() >= MAPPED_CELLS {
            held.old = std::mem::take(&mut held.young);
        }
        held.young.insert(id, mapped.clone());
        mapped
    }

    /// Every system within `radius` light years of `at`, with its place and
    /// its true distance.
    ///
    /// The cells come off the tree by descent ([`Index::each_near`]) so the
    /// work is the sphere's and not the galaxy's, and each one's systems are
    /// measured out of its mapping. A cell straddling the sphere is scanned
    /// and its systems tested, which is why this and not the cell set is
    /// what a caller wants.
    pub fn each_near(
        &self,
        at: [f64; 3],
        radius: f64,
        mut found: impl FnMut(Node, [f64; 3], f64),
    ) {
        let mut cells = Vec::new();
        self.index.each_near(at, radius, |id| cells.push(id));
        let reach = radius * radius;
        for cell in cells {
            let Some(payload) = self.payload(cell) else { continue };
            for i in 0..payload.len() {
                let place = payload.position_at(i);
                let away = dist2(at, place);
                if away <= reach {
                    found(
                        Node { cell, at: i as u32 },
                        place,
                        away.sqrt(),
                    );
                }
            }
        }
    }

    /// Where a node sits.
    pub fn place(&self, node: Node) -> Option<[f64; 3]> {
        let payload = self.payload(node.cell)?;
        ((node.at as usize) < payload.len())
            .then(|| payload.position_at(node.at as usize))
    }

    /// What a node's system is called by address.
    pub fn address(&self, node: Node) -> Option<i64> {
        let payload = self.payload(node.cell)?;
        ((node.at as usize) < payload.len())
            .then(|| payload.id64_at(node.at as usize) as i64)
    }

    /// Which node holds `address`, given somewhere near where it sits.
    ///
    /// The one query that cannot start from a position, because it starts
    /// from a system a commander named — so the place comes from the names
    /// table and this finds the record. A handful per route, at its ends,
    /// which is why searching a small neighbourhood is affordable where
    /// an address-to-cell index over the galaxy would not be worth its
    /// bytes.
    ///
    /// The radius widens until something is found or the galaxy is
    /// exhausted: a position carried at `f32` in the names table and at
    /// `f64` in the payload agree to within a light year or so, but a cell
    /// boundary can fall between them.
    pub fn node_of(&self, address: i64, near: [f64; 3]) -> Option<Node> {
        let wanted = address as u64;
        let mut radius = 1.0;
        while radius < crate::geometry::ROOT_EDGE_LY {
            let mut found = None;
            self.each_near(near, radius, |node, _, _| {
                if found.is_none()
                    && self.address(node) == Some(wanted as i64)
                {
                    found = Some(node);
                }
            });
            if found.is_some() {
                return found;
            }
            radius *= 8.0;
        }
        None
    }
}

/// The squared distance between two places, in light years.
fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{BuildParams, Snapshot, System};

    /// A scratch directory removed with the test.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let at = std::env::temp_dir()
            .join(format!("galos-sky-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        Scratch(at)
    }

    /// A lattice of systems, one every `step` light years on a side.
    fn lattice(side: usize, step: f64) -> Vec<System> {
        let span = (side.saturating_sub(1)) as f64 * step;
        let base = [-span / 2.0, 900.0 - span / 2.0, 24400.0 - span / 2.0];
        let mut out = Vec::new();
        let mut id = 1u64;
        for x in 0..side {
            for y in 0..side {
                for z in 0..side {
                    out.push(System {
                        id64: id,
                        position: [
                            base[0] + x as f64 * step,
                            base[1] + y as f64 * step,
                            base[2] + z as f64 * step,
                        ],
                        absolute_magnitude: id as f64 * 0.001 - 3.0,
                        temperature: 5000.0,
                        age_bucket: 0,
                        updated_at: 0,
                    });
                    id += 1;
                }
            }
        }
        out
    }

    fn built(dir: &Path, systems: &[System]) {
        Snapshot::build(systems, &BuildParams::default())
            .write(dir)
            .expect("a build");
    }

    /// A neighbour query answers every system in the sphere and nothing
    /// outside it, whatever level the cells that hold them sit at.
    ///
    /// The additive slices are the risk: a cell owns only what its
    /// ancestors did not, so the brightest systems in reach are held high
    /// up the tree and a walk that stopped at leaves would miss them.
    #[test]
    fn a_query_answers_the_sphere_and_nothing_else() {
        let dir = scratch("sphere");
        let systems = lattice(12, 40.0);
        built(&dir.0, &systems);
        let sky = Sky::open(&dir.0).expect("the sky opens");
        assert_eq!(sky.len(), systems.len() as u64);

        let at = systems[systems.len() / 2].position;
        for radius in [0.0, 41.0, 100.0, 250.0] {
            let mut found: Vec<u64> = Vec::new();
            sky.each_near(at, radius, |node, place, away| {
                assert!(away <= radius + 1e-6, "{away} past {radius}");
                assert_eq!(sky.place(node), Some(place));
                found.push(sky.address(node).expect("an address") as u64);
            });
            found.sort_unstable();

            let mut wanted: Vec<u64> = systems
                .iter()
                .filter(|it| dist2(at, it.position) <= radius * radius)
                .map(|it| it.id64)
                .collect();
            wanted.sort_unstable();
            assert_eq!(found, wanted, "at radius {radius}");
        }
    }

    /// A system is found by address from the place the names table carries,
    /// which is `f32` where the payload's is `f64`.
    #[test]
    fn a_named_system_is_found_from_its_place() {
        let dir = scratch("named");
        let systems = lattice(8, 40.0);
        built(&dir.0, &systems);
        let sky = Sky::open(&dir.0).expect("the sky opens");

        for system in [&systems[0], &systems[systems.len() / 3]] {
            // The place as the names table would hand it over: narrowed to
            // `f32` and widened back.
            let near = [
                system.position[0] as f32 as f64,
                system.position[1] as f32 as f64,
                system.position[2] as f32 as f64,
            ];
            let node = sky
                .node_of(system.id64 as i64, near)
                .expect("the system is in the sky");
            assert_eq!(sky.address(node), Some(system.id64 as i64));
            assert_eq!(sky.place(node), Some(system.position));
        }

        // An address nothing holds is not found, and does not hang looking.
        assert_eq!(sky.node_of(-1, systems[0].position), None);
    }

    /// The mapped set is bounded, and a query that sweeps past the bound
    /// still answers.
    #[test]
    fn the_mapped_set_is_bounded() {
        let dir = scratch("bounded");
        let systems = lattice(14, 40.0);
        built(&dir.0, &systems);
        let sky = Sky::open(&dir.0).expect("the sky opens");

        // Every cell there is, which is more than a corridor and fewer than
        // the cap — enough to prove the two generations hand mappings over
        // rather than losing them.
        let whole = crate::geometry::ROOT_EDGE_LY;
        let mut count = 0;
        sky.each_near([0.0, 900.0, 24400.0], whole, |_, _, _| count += 1);
        assert_eq!(count, systems.len());

        let held = sky.held.lock().expect("the mapped payloads");
        assert!(
            held.young.len() + held.old.len() <= 2 * MAPPED_CELLS,
            "the mapped set is unbounded",
        );
    }
}
