//! What the merge frontier draws, measured and rendered, off a built index.
//!
//! ```sh
//! cargo build --release --example frontier -p galos_index
//! ./target/release/examples/frontier .index/full /tmp/frontier
//! ```
//!
//! **The picture is the check, and a profile is not.** The attempt this
//! replaced passed a full battery of per-axis profile measurements and still
//! drew hard-edged cubes, because a profile averages a bright box and a dark
//! box into a reasonable number. So this renders the mark layer itself — the
//! merged marks and the systems read out of the cells above them, and
//! nothing else, since a hole in the marks is what is being looked for — at
//! every zoom, plus a close-up straddling a cell face at each.
//!
//! The field is deliberately absent. It is the half of the map that was
//! never patchy, and leaving it out is what makes a gap in the marks
//! visible. Everything is inside the spyglass reach the client would set
//! from that distance, so the counts are the ones a frame would draw.
//!
//! Two images a lens: `<zoom>.png`, the marks as the client draws them,
//! and `<zoom>-tinted.png`, the same with the merged marks in cyan and the
//! systems read out of the cells above them in white, which is where the
//! frontier sits.
//!
//! **What the picture has to show is density.** Every cell draws the same
//! share of what it *holds*, so the arms, the core and the voids come out
//! at the densities they have. The two rules this replaced both flattened
//! it: a share of a cell's screen footprint draws the same count over the
//! same patch whatever is in it, and a floor under that figure drew nothing
//! at all in the finest cells — which is where the sky is densest.

use galos_index::walk::{MERGE_PX, Mode, View};
use galos_index::{CellId, Index};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The frame every figure is quoted in: the client's own 1280x720 logical
/// viewport at 45 degrees, which is 869 pixels to the radian.
const WIDE: usize = 1280;
const HIGH: usize = 720;
const FOV_Y: f32 = std::f32::consts::FRAC_PI_4;

/// Where the camera is pointed: Sol, which is the map's origin and where a
/// user's camera starts.
const LOOK_AT: [f64; 3] = [0.0, 0.0, 0.0];

/// The zooms the plan quotes, light years back from [`LOOK_AT`].
const ZOOMS: [f64; 6] = [971.0, 2_000.0, 8_000.0, 30_000.0, 60_000.0, 130_000.0];

/// A camera `back` light years from `at`, looking at it, through `fov_y`.
fn looking(at: [f64; 3], back: f64, fov_y: f32) -> View {
    View {
        eye: [at[0], at[1], at[2] - back],
        forward: [0.0, 0.0, 1.0],
        up: [0.0, 1.0, 0.0],
        fov_y,
        viewport_height: HIGH as f32,
        aspect: WIDE as f32 / HIGH as f32,
    }
}

/// The camera's basis and focal length: what turns a light-year position
/// into a pixel.
struct Lens {
    eye: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
    forward: [f64; 3],
    focal: f64,
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Lens {
    fn of(view: &View) -> Lens {
        let forward = unit(view.forward);
        let right = unit(cross(forward, view.up));
        let up = cross(right, forward);
        let focal =
            HIGH as f64 / 2.0 / (f64::from(view.fov_y) / 2.0).tan();
        Lens { eye: view.eye, right, up, forward, focal }
    }

    /// Where a position lands, or [`None`] behind the camera.
    fn at(&self, p: [f64; 3]) -> Option<(f64, f64)> {
        let v = [p[0] - self.eye[0], p[1] - self.eye[1], p[2] - self.eye[2]];
        let ahead = dot(v, self.forward);
        if ahead <= 0.0 {
            return None;
        }
        let x = dot(v, self.right) / ahead * self.focal;
        let y = dot(v, self.up) / ahead * self.focal;
        Some((WIDE as f64 / 2.0 + x, HIGH as f64 / 2.0 - y))
    }

    /// The same eye through a lens `times` as long, aimed at `at`: a
    /// magnifier held over the picture, not a second picture. The drawn set
    /// is whatever the walk answered for the eye this was built from.
    fn magnified(self, at: [f64; 3], times: f64) -> Lens {
        let forward = unit([
            at[0] - self.eye[0],
            at[1] - self.eye[1],
            at[2] - self.eye[2],
        ]);
        let right = unit(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        Lens { forward, right, up, focal: self.focal * times, ..self }
    }
}

/// Light laid down, one f32 a channel a pixel.
struct Canvas {
    light: Vec<[f32; 3]>,
}

impl Canvas {
    fn new() -> Canvas {
        Canvas { light: vec![[0.0; 3]; WIDE * HIGH] }
    }

    /// One mark: a disc of `radius` pixels, its rim a pixel of fade so the
    /// picture is not a lattice of squares.
    fn dot(&mut self, at: (f64, f64), radius: f64, tint: [f32; 3]) {
        let (cx, cy) = at;
        let reach = radius + 1.0;
        let lo_x = (cx - reach).floor().max(0.0) as usize;
        let hi_x = ((cx + reach).ceil() as isize).clamp(0, WIDE as isize);
        let lo_y = (cy - reach).floor().max(0.0) as usize;
        let hi_y = ((cy + reach).ceil() as isize).clamp(0, HIGH as isize);
        for y in lo_y..hi_y as usize {
            for x in lo_x..hi_x as usize {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let cover = (radius + 0.5 - d).clamp(0.0, 1.0) as f32;
                if cover <= 0.0 {
                    continue;
                }
                let pixel = &mut self.light[y * WIDE + x];
                for (channel, add) in pixel.iter_mut().zip(tint) {
                    *channel += add * cover;
                }
            }
        }
    }

    /// Eight-bit RGB: the light rolled off so an overlap saturates rather
    /// than clips, then gamma.
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(WIDE * HIGH * 3);
        for pixel in &self.light {
            for channel in pixel {
                let tone = channel / (1.0 + channel);
                out.push((tone.powf(1.0 / 2.2) * 255.0).round() as u8);
            }
        }
        out
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c = table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut framed = kind.to_vec();
    framed.extend_from_slice(body);
    out.extend_from_slice(&framed);
    out.extend_from_slice(&crc32(&framed).to_be_bytes());
}

/// A PNG of the canvas, deflate-stored so nothing but the standard library
/// is needed to write it.
fn png(rgb: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(HIGH * (1 + WIDE * 3));
    for y in 0..HIGH {
        raw.push(0);
        raw.extend_from_slice(&rgb[y * WIDE * 3..(y + 1) * WIDE * 3]);
    }
    let mut zlib = vec![0x78, 0x01];
    for (n, block) in raw.chunks(65_535).enumerate() {
        let last = u8::from((n + 1) * 65_535 >= raw.len());
        zlib.push(last);
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut head = Vec::new();
    head.extend_from_slice(&(WIDE as u32).to_be_bytes());
    head.extend_from_slice(&(HIGH as u32).to_be_bytes());
    head.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &head);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// What one level of the tree came to.
#[derive(Clone, Copy, Default)]
struct Row {
    cells: usize,
    read: usize,
    points: usize,
    drawn: usize,
    blobs: usize,
}

/// What one view came to.
#[derive(Default)]
struct Tally {
    /// The same, level by level.
    levels: [Row; 21],
    /// Cells whose payload the draw reads.
    read: usize,
    /// Systems in those payloads.
    points: usize,
    /// Marks actually drawn out of them.
    drawn: usize,
    /// Cells drawn as one merged mark.
    blobs: usize,
    /// Systems those merged marks stand for.
    behind: u64,
    /// Cells the field splats.
    splats: usize,
}

/// Walk, read, and lay the mark layer down, once per lens.
///
/// One walk and one read however many lenses: the drawn set is a fact about
/// where the eye stands, so a close-up is the same set through a longer
/// lens rather than a second plan.
///
/// `reach` is the spyglass: the map clears away what its bubble does not
/// hold, so a cell whose box is further than that from the camera's target
/// is neither fetched nor drawn. The client sets it off the camera — see
/// `galos_map`'s `reach_with_camera` — and every figure here is inside it,
/// as the client's are.
fn frame(
    index: &Index,
    dir: &Path,
    view: &View,
    reach: f64,
    lenses: &[Lens],
) -> Vec<Shot> {
    let mut tally = Tally::default();
    let at_ly = LOOK_AT;
    let inside = |id: CellId| id.bounds().distance_to(at_ly) <= reach;

    let at = Instant::now();
    let needed = index.needed(view, Mode::Shell, None);
    let walked = at.elapsed();
    tally.splats = needed.splats.len();

    // What the frame is spread over, and the one share struck across it.
    let mut population = 0u64;
    for mark in &needed.marks {
        if inside(mark.id) {
            population += u64::from(mark.slice);
        }
    }
    for blob in &needed.blobs {
        if inside(blob.id) {
            population += blob.count;
        }
    }
    let share = (frame_marks() / population.max(1) as f64).min(1.0);

    let mut marks: Vec<Mark> = Vec::new();
    for blob in &needed.blobs {
        if !inside(blob.id) {
            continue;
        }
        let Some(cell) = index.get(blob.id) else { continue };
        let Some(at) = cell.aggregate.count_centroid() else { continue };
        let count = blob.count;
        if wanted(share * blob.blend, count as usize, blob.id) == 0 {
            continue;
        }
        tally.blobs += 1;
        tally.behind += count;
        tally.levels[blob.id.level as usize].blobs += 1;
        marks.push(Mark { at, merged: true });
    }

    let at = Instant::now();
    for mark in &needed.marks {
        let id = mark.id;
        if !inside(id) {
            continue;
        }
        let take = wanted(share, mark.slice as usize, id);
        if take == 0 {
            continue;
        }
        let Ok(payload) = Index::read_payload_prefix(dir, id, take) else {
            continue;
        };
        let row = &mut tally.levels[id.level as usize];
        row.cells += 1;
        row.read += 1;
        row.points += payload.len();
        tally.read += 1;
        tally.points += payload.len();
        for point in &payload {
            let off = [
                point.pos[0] - at_ly[0],
                point.pos[1] - at_ly[1],
                point.pos[2] - at_ly[2],
            ];
            if dot(off, off) > reach * reach {
                continue;
            }
            tally.drawn += 1;
            tally.levels[id.level as usize].drawn += 1;
            marks.push(Mark { at: point.pos, merged: false });
        }
    }
    let read = at.elapsed();

    println!("  level   cells      read    points      marks     blobs");
    for level in 0..=20u8 {
        let row = &tally.levels[level as usize];
        if row.cells == 0 && row.blobs == 0 {
            continue;
        }
        println!(
            "  {level:>5} {:>7} {:>9} {:>9} {:>10} {:>9}",
            row.cells, row.read, row.points, row.drawn, row.blobs,
        );
    }

    println!(
        "  walk {walked:>8.2?}  read {read:>8.2?}  share {share:.6}  \
         over {population:>11}  cells read {:>7}  points {:>9}  \
         marks {:>7}  blobs {:>7} over {:>11}  splats {:>7}",
        tally.read,
        tally.points,
        tally.drawn,
        tally.blobs,
        tally.behind,
        tally.splats,
    );

    lenses.iter().map(|lens| paint(lens, &marks)).collect()
}

/// How many marks a 1280x720 frame carries at the merge distance, which is
/// `galos_map`'s `bounded::frame_marks`.
fn frame_marks() -> f64 {
    (WIDE * HIGH) as f64 / (MERGE_PX * MERGE_PX)
}

/// How many marks a cell of `held` systems draws at `share`, dithered
/// against its own address so a fraction of a mark is drawn in a fraction
/// of the cells rather than nowhere.
///
/// `galos_map`'s `bounded::wanted`, replicated so this renders exactly what
/// the client does. Both are a share of *population*: a region with ten
/// times the systems draws ten times the marks, which is the whole of the
/// density response and the one thing a share of a cell's screen footprint
/// cannot say.
fn wanted(share: f64, held: usize, id: CellId) -> usize {
    let mut z = id.morton().wrapping_add(u64::from(id.level));
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let dither = (z >> 40) as f64 / 16_777_216.0;
    ((share * held as f64 + dither) as usize).min(held)
}

/// One mark the frame draws.
struct Mark {
    at: [f64; 3],
    /// Whether it stands for a whole merged cell rather than for a system.
    /// Drawn the same size either way: a blob is one mark because
    /// everything it holds falls inside one, not because it is worth more
    /// than one.
    merged: bool,
}

/// The pictures one lens makes of the frame's marks.
struct Shot {
    /// The marks as drawn.
    marks: Canvas,
    /// The same with the merged marks tinted, so the frontier shows.
    tinted: Canvas,
}

/// Lay the marks down through one lens.
fn paint(lens: &Lens, marks: &[Mark]) -> Shot {
    let mut drawn = Canvas::new();
    let mut tinted = Canvas::new();
    for mark in marks {
        let Some(at) = lens.at(mark.at) else { continue };
        drawn.dot(at, 0.75, [1.0; 3]);
        let tint = if mark.merged { [0.0, 1.0, 1.0] } else { [1.0; 3] };
        tinted.dot(at, 0.75, tint);
    }
    Shot { marks: drawn, tinted }
}

/// A point on a cell face at `level`, near where the camera is looking: the
/// close-ups straddle a boundary rather than sitting inside one box.
fn face_at(level: u8) -> [f64; 3] {
    let corner = CellId::of_point(LOOK_AT, level).bounds().min;
    [corner[0], corner[1], corner[2]]
}

/// The level whose cells are about one mark wide from `back` light years
/// out: where the frontier sits, and so which face a close-up must cross.
fn frontier_level(view: &View, back: f64) -> u8 {
    let want = MERGE_PX * back / view.pixels_per_radian();
    (0..=20u8)
        .find(|&level| galos_index::geometry::edge_ly(level) <= want)
        .unwrap_or(20)
}

fn write(dir: &Path, name: &str, canvas: &Canvas) {
    let path = dir.join(format!("{name}.png"));
    std::fs::write(&path, png(&canvas.bytes())).expect("the image writes");
    println!("  {}", path.display());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(
        args.next().unwrap_or_else(|| ".index/full".to_string()),
    );
    let out = PathBuf::from(
        args.next().unwrap_or_else(|| "/tmp/frontier".to_string()),
    );
    std::fs::create_dir_all(&out).expect("an output directory");

    let at = Instant::now();
    let index = Index::read(&dir).expect("the index should read");
    println!(
        "{} cells, {} systems, read in {:.2?}",
        index.len(),
        index.root().map_or(0, |root| root.aggregate.count()),
        at.elapsed(),
    );

    for back in ZOOMS {
        let view = looking(LOOK_AT, back, FOV_Y);
        let reach = reach_of(&view, back);
        println!("\n{back:.0} ly back, reach {reach:.0} ly:");
        let wide = Lens::of(&view);

        // And the same drawn set through a lens eight times as long, aimed
        // at a corner of the frontier's own cells. The *walk* is the one
        // above — same marks, same blobs, same frontier — so this is a
        // magnifier held over the picture and not a second picture, which
        // is what makes it a check on the cell face rather than on a
        // different level of detail.
        let level = frontier_level(&view, back);
        let close = Lens::of(&view).magnified(face_at(level), 8.0);
        println!(
            "  close-up across a level-{level} face ({:.0} ly cells)",
            galos_index::geometry::edge_ly(level),
        );

        let shots = frame(&index, &dir, &view, reach, &[wide, close]);
        for (what, shot) in [("", &shots[0]), ("-face", &shots[1])] {
            write(&out, &format!("{back:.0}{what}"), &shot.marks);
            write(&out, &format!("{back:.0}{what}-tinted"), &shot.tinted);
        }
    }
}

/// How far the client's spyglass reaches from `back` light years out: what
/// the camera takes in, less the margin it holds off by.
///
/// `galos_map`'s `reach_with_camera` and `camera::framed`, read off here so
/// every figure is the one the client would draw. Ten per cent short of the
/// frame, and the galaxy's own edge past that.
fn reach_of(view: &View, back: f64) -> f64 {
    let seen = back * (f64::from(view.fov_y) / 2.0).tan();
    (seen * 0.9).min(f64::from(65_000.0))
}
