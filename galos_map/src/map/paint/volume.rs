//! The glow's evenly filled cells, ray-marched as one volume
//!
//! **Why not a splat.** Most of the galaxy stands in index leaves 256 to 512
//! light years across, each evenly filled, and the index can say no more of
//! one than its count: measured over `.index/full`, 69 % of the systems stand
//! in leaves 256 light years wide or wider. [`crate::map::paint::glow`] lays
//! each as one Gaussian at its centroid, so a view where those leaves stand
//! tens of pixels apart draws a regular grid of lumps on their pitch, and a
//! seam wherever the cells change size. Splatting cannot be flat across a
//! change of size: a pixel sums its splats over depth, and what would make
//! them flat is a weight normalised at each *point* of space, not at each
//! pixel.
//!
//! **The basis method** (Wald et al., *CPU Volume Rendering of Adaptive Mesh
//! Refinement Data*, 2017). The density at a point is a weighted mean of the
//! cells near it, `ρ(x) = Σ wᵢ(x) ρᵢ / Σ wᵢ(x)`, each `wᵢ` a tent as wide as
//! its own cell about its centre: one there, nothing at the centres of the
//! cells of its size beside it. Between cells of one size that is trilinear
//! interpolation of their densities; where sizes change it is still a mean,
//! so an even crowd stays exactly flat across the change, and it is
//! continuous everywhere because every weight is.
//!
//! **Ghosts say where the crowd stops.** A mean of what is there is flat out
//! to where the last tent ends and then nothing, which is an edge. So every
//! place beside a cell of the volume that holds none of it is a cell of
//! density nought ([`Built::of`]), and the mean falls to nothing across it
//! over one cell. A ghost is laid at the size of the cell it stands beside,
//! and finer where finer cells of the volume are beside it, so its weight
//! does not reach into them.
//!
//! **Marched a pixel at a time.** The cells and the ghosts tile the volume
//! without overlapping, as boxes. One quad covers them, and each pixel walks
//! its ray from box to box — found in a table of every box and every cell
//! holding one, by their places in the cube — integrating the field through
//! each, and over empty cells whole. Each box carries the list of cells
//! whose tents reach into it and which of its eight octants each reaches,
//! which is everything a point inside it needs and nothing else. See
//! `volume.wgsl`.
//!
//! Summed in the shader and laid down once. Drawn instead as a quad a box,
//! summed by the blend, each pixel took a few dozen half-float roundings
//! that stepped wherever a box's edge crossed the frame. And within a box
//! the ray is cut wherever the field can turn, where a cell on its list
//! centres or its tent ends: integrated across those turns, the error moved
//! with where the ray crossed them and drew lines a percent deep.
//!
//! **At half the frame's resolution**, into targets of their own that
//! [`crate::map::paint::curve`] samples back up and adds to the field's:
//! the field is smooth by construction, and the march is the dearest thing
//! the map draws.
//!
//! **Handed over by screen size.** A cell a pixel or two across is
//! better as a Gaussian: half a pixel is the floor a splat is laid at, which
//! filters what a pixel cannot show, where a volume sampled along each
//! pixel's middle ray would alias. So a cell's light moves from its splat
//! into the volume across [`FROM_PX`]..[`FULL_PX`] of its edge on screen
//! ([`share`]), the two together carrying it whole all the way.
//!
//! **Only where the systems fill the cell** ([`FILLED`]), which is where the
//! cell holds nothing finer than itself to lose. A filament or a knot is a
//! cell whose systems sit tighter than it, and keeps its Gaussian about its
//! own centroid at its own spread; that is where the detail is.

use crate::map::camera::{OrbitCamera, VOLUME_DIMMED_LAYER, VOLUME_LAYER};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType,
    SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use bevy::tasks::ComputeTaskPool;
use galos_index::core::geometry::MAX_LEVEL;
use galos_index::prelude::CellId;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::hash_map::Entry;

pub fn plugin(app: &mut App) {
    embedded_asset!(app, "volume.wgsl");
    app.add_plugins(MaterialPlugin::<VolumeLight>::default());
    app.add_systems(Startup, spawn_volume);
}

/// How far a channel's systems have to spread over its cell, as a share of
/// the cell's edge, for it to be laid into the volume
///
/// An evenly filled cube's RMS radius is half its edge; measured over
/// `.index/full`, the median leaf at every level from 512 light years down
/// stands at 0.48 or 0.49. The sparse leaves of 2048 light years and wider at
/// the rim stand at 0.32 to 0.39, a few hundred systems each standing where
/// they happen to, and a cell holding a filament or a knot well under that:
/// those keep their Gaussian about their own centroid at their own spread.
pub(crate) const FILLED: f64 = 0.42;

/// The edge on screen, in logical pixels, at which a filled cell starts
/// handing its light from its splat to the volume
///
/// Under it the cell is a few pixels, where Gaussians half a cell wide over
/// the pixel grid sum flat and filter what a pixel cannot show; measured
/// with the galaxy seen whole, where the leaves stand three pixels apart,
/// the splats' ripple at their pitch was a quarter of a percent.
pub(crate) const FROM_PX: f32 = 4.;

/// And where the volume carries a filled cell's light alone
///
/// An octave over [`FROM_PX`], the band the walk cross-fades its own levels
/// across. It also keeps the volume's boxes clear of the pixel: a box is
/// integrated along the ray through each pixel's middle, and boxes much
/// under a pixel would be sampled rather than covered.
pub(crate) const FULL_PX: f32 = 8.;

/// How much of a filled cell's light the volume carries, by the cell's edge
/// on screen in logical pixels; its splat carries the rest
///
/// Smooth in the edge, so a cell handing over as the camera moves does not
/// move the light's shape in steps.
pub(crate) fn share(edge_px: f32) -> f32 {
    if edge_px.is_nan() {
        return 0.;
    }
    let t = ((edge_px - FROM_PX) / (FULL_PX - FROM_PX)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// How wide the volume's edge is where the spyglass clears, as a share of the
/// reach
///
/// The volume is clipped to the reach's sphere and fades out over this much
/// of it inside, where a splat fades by how much of itself the reach still
/// holds. A sixteenth: a cell or two at the zooms the reach is set from.
const SOFT: f32 = 1. / 16.;

/// Where an entry of a box's list keeps the octants it reaches: the high
/// byte, the low three holding the cell.
const OCTANTS: u32 = 24;

/// The cell an entry of a box's list names, under its octants.
const CELL: u32 = (1 << OCTANTS) - 1;

/// How many cells one thread works through: enough that a chunk is not
/// mostly overhead, few enough that a view of twenty thousand cells is
/// spread over every core.
const CHUNK: usize = 1024;

/// One cell's light laid into the volume: the let-through and what the
/// filters exclude, the light its systems lay, not yet a density
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Source {
    pub(crate) id: CellId,
    pub(crate) lit: Vec3,
    pub(crate) dimmed: Vec3,
}

/// The frame's volume as the shader takes it
///
/// Every cell of the volume, the cells that carry light first and the
/// ghosts after them, as its box from the eye and its density in each
/// target; each one's list of the cells whose tents reach into its box; a
/// table to find the box a point is in by; and the bounds of them all.
#[derive(Default)]
pub(crate) struct Built {
    /// Centre from the eye and edge, light years.
    boxes: Vec<[f32; 4]>,
    /// Light per cubic light year, let-through and dimmed; nought for a
    /// ghost.
    density: [Vec<[f32; 4]>; 2],
    /// Where each box's list starts in `influence`; and how long it is
    /// under [`LISTED`], with a bit over it for each target the box has any
    /// light to lay in ([`LIGHTS`]).
    spans: Vec<[u32; 2]>,
    /// A cell under [`CELL`] and the octants of the box it reaches over
    /// [`OCTANTS`].
    influence: Vec<u32>,
    /// Every box, and every cell holding one, by its place: see [`Table`].
    table: Vec<[u32; 4]>,
    /// The corners of the box every box stands in, from the eye.
    low: Vec3,
    high: Vec3,
    /// Whether each target has any light to lay.
    lights: [bool; 2],
}

impl Built {
    /// The volume of `sources`, seen from `eye`
    ///
    /// **Cells.** Coarse first, so a cell whose parent is already in carries
    /// its light into the parent: the walk's cross-fade lays both, and the
    /// boxes must not overlap.
    ///
    /// **Ghosts.** Each cell's 26 neighbours of its own size that the volume
    /// does not hold, coarse first: one inside a cell already held is
    /// skipped; one that holds finer cells of the volume, or stands face to
    /// face with one that does, is split into its eight and those are asked
    /// again a level down; the rest are ghosts. So the ghosts and the cells
    /// tile without overlapping, and a ghost is never coarser than the cells
    /// its tent would reach into.
    ///
    /// **Lists.** Every cell and ghost finds the boxes its tent reaches
    /// into: its own and its neighbours' if held at its size, the coarser box
    /// holding a neighbour, or the finer boxes inside one. A tent reaches
    /// exactly as far as the neighbours' centres, and the 27 boxes about a
    /// cell hold all of that.
    pub(crate) fn of(sources: &[Source], eye: DVec3) -> Built {
        if sources.is_empty() {
            return Built::default();
        }
        let mut ranked: Vec<&Source> = sources.iter().collect();
        ranked.sort_by_key(|source| source.id.level);

        let mut held = Places::default();
        held.reserve(sources.len() * 2);
        let mut cells: Vec<CellId> = Vec::with_capacity(sources.len() * 2);
        let mut light: Vec<[Vec3; 2]> = Vec::with_capacity(sources.len() * 2);
        for source in ranked {
            let into = held.get(&key(source.id)).copied().or_else(|| {
                let parent = source.id.parent()?;
                held.get(&key(parent)).copied()
            });
            if let Some(at) = into {
                light[at as usize][0] += source.lit;
                light[at as usize][1] += source.dimmed;
                continue;
            }
            held.insert(key(source.id), cells.len() as u32);
            cells.push(source.id);
            light.push([source.lit, source.dimmed]);
        }
        let lit = cells.len();

        // What holds a cell of the volume further down: the strict
        // ancestors of every one that carries light.
        let mut under = Keys::default();
        ancestors(&cells, &mut under);

        // The places beside a cell that the volume does not hold, then a
        // level at a time, coarse first: each place's verdict reads only
        // what is held coarser than it and the cells under it, so a level's
        // are asked together.
        let mut waiting: Vec<Vec<CellId>> =
            vec![Vec::new(); usize::from(MAX_LEVEL) + 1];
        for beside in spread(lit, |range| {
            let mut out = Vec::new();
            for &id in &cells[range] {
                out.extend(
                    around(id).filter(|near| !held.contains_key(&key(*near))),
                );
            }
            out
        }) {
            for id in beside {
                waiting[usize::from(id.level)].push(id);
            }
        }
        let mut split = Keys::default();
        for level in 0..=usize::from(MAX_LEVEL) {
            let now = std::mem::take(&mut waiting[level]);
            if now.is_empty() {
                continue;
            }
            let verdicts = spread(now.len(), |range| {
                now[range]
                    .iter()
                    .map(|&id| verdict(&held, &under, id))
                    .collect::<Vec<_>>()
            });
            for (&id, verdict) in now.iter().zip(verdicts.into_iter().flatten())
            {
                match verdict {
                    Verdict::Held => {}
                    Verdict::Split => {
                        if split.insert(key(id)) {
                            waiting[level + 1].extend(id.children());
                        }
                    }
                    Verdict::Ghost => {
                        if let Entry::Vacant(place) = held.entry(key(id)) {
                            place.insert(cells.len() as u32);
                            cells.push(id);
                            light.push([Vec3::ZERO; 2]);
                        }
                    }
                }
            }
        }
        debug_assert!(
            cells.len() <= CELL as usize,
            "a box list names a cell in three bytes"
        );

        // And now of the ghosts too, for finding the boxes inside a place.
        let mut holds = under;
        ancestors(&cells[lit..], &mut holds);

        let n = cells.len();
        let found = spread(n, |range| {
            let mut out = Vec::new();
            for cell in range {
                reached(&held, &holds, &cells, cell as u32, &mut out);
            }
            out
        });

        // Each box's list, gathered: a count, a running sum, and a fill. And
        // which boxes a target draws: those a cell with light in it reaches.
        let shines: [Vec<bool>; 2] = std::array::from_fn(|target| {
            light.iter().map(|light| light[target].max_element() > 0.).collect()
        });
        let mut spans = vec![[0u32; 2]; n];
        let mut lit_by = vec![0u32; n];
        let mut finer = vec![0u8; n];
        for &(region, entry) in found.iter().flatten() {
            let k = (entry & CELL) as usize;
            spans[region as usize][1] += 1;
            for (target, shines) in shines.iter().enumerate() {
                lit_by[region as usize] |= u32::from(shines[k]) << target;
            }
            let below =
                cells[k].level.saturating_sub(cells[region as usize].level);
            finer[region as usize] = finer[region as usize].max(below);
        }
        let mut next = 0u32;
        for span in &mut spans {
            span[0] = next;
            next += span[1];
        }
        let mut filled: Vec<u32> = spans.iter().map(|span| span[0]).collect();
        let mut influence = vec![0u32; next as usize];
        for &(region, entry) in found.iter().flatten() {
            let at = &mut filled[region as usize];
            influence[*at as usize] = entry;
            *at += 1;
        }
        for ((span, lit_by), finer) in spans.iter_mut().zip(&lit_by).zip(&finer)
        {
            debug_assert!(
                span[1] <= LISTED,
                "a box's list is counted in three bytes"
            );
            span[1] |=
                u32::from((*finer).min(FINEST)) << FINER | lit_by << LIGHTS;
        }
        let lights =
            [0, 1].map(|target| lit_by.iter().any(|by| by >> target & 1 == 1));

        let density: [Vec<[f32; 4]>; 2] = std::array::from_fn(|target| {
            cells
                .iter()
                .zip(&light)
                .map(|(id, light)| {
                    let rho = light[target] / id.edge_ly().powi(3) as f32;
                    [rho.x, rho.y, rho.z, 0.]
                })
                .collect()
        });
        let (mut low, mut high) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
        let boxes = cells
            .iter()
            .map(|id| {
                let bounds = id.bounds();
                low = low.min(DVec3::from(bounds.min));
                high = high.max(DVec3::from(bounds.max));
                let centre = DVec3::from(bounds.center()) - eye;
                [
                    centre.x as f32,
                    centre.y as f32,
                    centre.z as f32,
                    id.edge_ly() as f32,
                ]
            })
            .collect();
        let mut table = Table::with_room(n + holds.len());
        for (at, &id) in cells.iter().enumerate() {
            table.put(id, at as u32);
            let mut up = id;
            while let Some(parent) = up.parent() {
                if !table.put(parent, Table::HOLDS) {
                    break;
                }
                up = parent;
            }
        }
        Built {
            boxes,
            density,
            spans,
            influence,
            table: table.slots,
            low: (low - eye).as_vec3(),
            high: (high - eye).as_vec3(),
            lights,
        }
    }
}

/// Where a span keeps the target bits: over its list's length and how much
/// finer its finest neighbour is.
const LIGHTS: u32 = 30;

/// Where a span keeps how many levels finer than its box the finest cell
/// on its list is, which says where in the box the field can turn.
const FINER: u32 = 24;

/// The most levels finer a box is cut for. Past it the pieces are left
/// longer than the turns in them, which the integration then rounds over.
const FINEST: u8 = 2;

/// How long a box's list is, out of its span.
const LISTED: u32 = (1 << FINER) - 1;

/// The cells of the volume by their place in the cube, and every cell
/// holding one, for the shader to find the box a point stands in
///
/// Open addressing with a linear probe, at most half full. A slot is the
/// place — `x`, `y`, and `z` with the level over it from bit 24 — and what
/// is there: the box's index, or [`Table::HOLDS`] for a cell that only
/// holds boxes further down. Every cell holding a box is in, so the levels
/// held at a point run unbroken from the root down to the box it is in, or
/// to the empty cell it is in, and the shader finds which by halving.
struct Table {
    slots: Vec<[u32; 4]>,
}

impl Table {
    /// What an empty slot holds where a place's `z` and level go: more than
    /// any level sets.
    const EMPTY: u32 = u32::MAX;

    /// What a cell holding boxes further down is put in as.
    const HOLDS: u32 = u32::MAX - 1;

    fn with_room(cells: usize) -> Table {
        let size = (cells * 2).next_power_of_two().max(16);
        Table { slots: vec![[0, 0, Table::EMPTY, 0]; size] }
    }

    /// Put `id` in as `what` where it is not in already, answering whether
    /// it was put in.
    fn put(&mut self, id: CellId, what: u32) -> bool {
        let mask = self.slots.len() as u32 - 1;
        let tagged = id.z | u32::from(id.level) << 24;
        let mut slot = scatter(id.x, id.y, tagged) & mask;
        loop {
            let at = &mut self.slots[slot as usize];
            if at[2] == Table::EMPTY {
                *at = [id.x, id.y, tagged, what];
                return true;
            }
            if at[0] == id.x && at[1] == id.y && at[2] == tagged {
                return false;
            }
            slot = (slot + 1) & mask;
        }
    }
}

/// Where a place starts its probe in a [`Table`]: `volume.wgsl`'s
/// `scatter`, the same arithmetic.
fn scatter(x: u32, y: u32, tagged: u32) -> u32 {
    let mut h = x.wrapping_mul(73_856_093)
        ^ y.wrapping_mul(19_349_663)
        ^ tagged.wrapping_mul(83_492_791);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ h >> 12
}

/// The volume's cells by [`key`], each to where it stands in the frame's list
type Places = FxHashMap<u64, u32>;

/// Cells by [`key`]
type Keys = FxHashSet<u64>;

/// A cell as the volume's maps key it: its coordinates packed under a bit
/// that says its level, so one word, and one multiply to hash
fn key(id: CellId) -> u64 {
    let level = u32::from(id.level);
    1u64 << (3 * level)
        | u64::from(id.x) << (2 * level)
        | u64::from(id.y) << level
        | u64::from(id.z)
}

/// `work` over `0..len` in chunks, across the compute pool where there are
/// enough of them to be worth it, the answers in order
fn spread<R: Send + 'static>(
    len: usize,
    work: impl Fn(std::ops::Range<usize>) -> R + Sync,
) -> Vec<R> {
    if len < CHUNK * 2 {
        return vec![work(0..len)];
    }
    let work = &work;
    // The app has stood the pool up long before; a test has not.
    ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default).scope(
        |scope| {
            for start in (0..len).step_by(CHUNK) {
                let end = (start + CHUNK).min(len);
                scope.spawn(async move { work(start..end) });
            }
        },
    )
}

/// The strict ancestors of `cells`, added to `into`
fn ancestors(cells: &[CellId], into: &mut Keys) {
    for &id in cells {
        let mut at = id;
        while let Some(parent) = at.parent() {
            if !into.insert(key(parent)) {
                break;
            }
            at = parent;
        }
    }
}

/// What a place beside the volume comes to
#[derive(Clone, Copy)]
enum Verdict {
    /// Held already, itself or by a coarser cell.
    Held,
    /// Finer cells of the volume are inside it or face to face with it, so
    /// it is asked again as its eight.
    Split,
    /// A ghost at its own size.
    Ghost,
}

/// What the place `id` beside the volume comes to; see [`Built::of`]
fn verdict(held: &Places, under: &Keys, id: CellId) -> Verdict {
    if held.contains_key(&key(id)) || held_above(held, under, id) {
        return Verdict::Held;
    }
    let finer = under.contains(&key(id))
        || faces(id).any(|near| under.contains(&key(near)));
    match finer && id.level < MAX_LEVEL {
        true => Verdict::Split,
        false => Verdict::Ghost,
    }
}

/// Whether a cell coarser than `id` is held over it
///
/// Up from `id` until a held cell answers yes, or an ancestor of held cells
/// answers no: the held cells do not overlap, so nothing over an ancestor of
/// one is held.
fn held_above(held: &Places, under: &Keys, id: CellId) -> bool {
    let mut at = id;
    while let Some(parent) = at.parent() {
        let parent_key = key(parent);
        if held.contains_key(&parent_key) {
            return true;
        }
        if under.contains(&parent_key) {
            return false;
        }
        at = parent;
    }
    false
}

/// The cell `(dx, dy, dz)` along from `id` at its own size, where the cube
/// has one
fn along(id: CellId, dx: i64, dy: i64, dz: i64) -> Option<CellId> {
    let last = (1i64 << id.level) - 1;
    let step = |at: u32, by: i64| {
        let to = i64::from(at) + by;
        (0..=last).contains(&to).then_some(to as u32)
    };
    Some(CellId {
        level: id.level,
        x: step(id.x, dx)?,
        y: step(id.y, dy)?,
        z: step(id.z, dz)?,
    })
}

/// The 27 cells of `id`'s size about it, itself among them, each with how
/// far along it stands
fn block(id: CellId) -> impl Iterator<Item = ([i64; 3], CellId)> {
    (0..27i64).filter_map(move |k| {
        let by = [k % 3 - 1, k / 3 % 3 - 1, k / 9 - 1];
        Some((by, along(id, by[0], by[1], by[2])?))
    })
}

/// The 26 about it, itself not among them
fn around(id: CellId) -> impl Iterator<Item = CellId> {
    block(id).filter(|(by, _)| *by != [0; 3]).map(|(_, near)| near)
}

/// Which octants of a box the tent of a cell its own size `by` from it
/// reaches: both halves along an axis it stands level on, and the half
/// facing it along one it stands off along
///
/// [`octants`] for the commonest case, without the arithmetic.
fn facing(by: [i64; 3]) -> u32 {
    let sides = by.map(|d| match d {
        0 => 0b11u32,
        d if d < 0 => 0b01,
        _ => 0b10,
    });
    (0..8u32)
        .filter(|o| {
            (0..3).all(|axis| sides[axis] >> ((o >> axis) & 1) & 1 == 1)
        })
        .fold(0, |mask, o| mask | 1 << o)
}

/// The six that share a face with it
fn faces(id: CellId) -> impl Iterator<Item = CellId> {
    [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
        .into_iter()
        .filter_map(move |(dx, dy, dz)| along(id, dx, dy, dz))
}

/// Which octants of `region`'s box the tent about `centre`, `edge` wide each
/// way, reaches into: bit `x | y << 1 | z << 2`, a one for the upper half
///
/// Open on both sides, since a tent is nothing at the end of its reach.
fn octants(region: CellId, centre: DVec3, edge: f64) -> u32 {
    let low = DVec3::from(region.min_ly());
    let half = region.edge_ly() * 0.5;
    let (from, to) = (centre - edge, centre + edge);
    let sides: [u32; 3] = std::array::from_fn(|axis| {
        let (a, m) = (low[axis], low[axis] + half);
        let b = m + half;
        u32::from(a < to[axis] && m > from[axis])
            | u32::from(m < to[axis] && b > from[axis]) << 1
    });
    (0..8u32)
        .filter(|o| {
            (0..3).all(|axis| sides[axis] >> ((o >> axis) & 1) & 1 == 1)
        })
        .fold(0, |mask, o| mask | 1 << o)
}

/// The boxes `cells[cell]`'s tent reaches into, pushed onto `out` as the box
/// and the entry its list takes for this cell
fn reached(
    held: &Places,
    holds: &Keys,
    cells: &[CellId],
    cell: u32,
    out: &mut Vec<(u32, u32)>,
) {
    let id = cells[cell as usize];
    let centre = DVec3::from(id.bounds().center());
    let edge = id.edge_ly();
    // A coarser box can hold several of the places about the cell; it is
    // listed once. A box of its own size is one place, and the finer boxes
    // under one place are under no other.
    let mut coarser: Vec<u32> = Vec::new();
    let mut below = Vec::new();
    for (by, place) in block(id) {
        if let Some(&region) = held.get(&key(place)) {
            // `region` is `by` from the cell, so the cell is `-by` from it.
            let mask = facing(by.map(|d| -d));
            out.push((region, cell | mask << OCTANTS));
        } else if holds.contains(&key(place)) {
            below.push(place);
            while let Some(at) = below.pop() {
                for child in at.children() {
                    let mask = octants(child, centre, edge);
                    if mask == 0 {
                        continue;
                    }
                    if let Some(&region) = held.get(&key(child)) {
                        out.push((region, cell | mask << OCTANTS));
                    } else if holds.contains(&key(child)) {
                        below.push(child);
                    }
                }
            }
        } else {
            let mut at = place;
            while let Some(parent) = at.parent() {
                if let Some(&region) = held.get(&key(parent)) {
                    if !coarser.contains(&region) {
                        coarser.push(region);
                        let mask = octants(parent, centre, edge);
                        if mask != 0 {
                            out.push((region, cell | mask << OCTANTS));
                        }
                    }
                    break;
                }
                if holds.contains(&key(parent)) {
                    break;
                }
                at = parent;
            }
        }
    }
}

/// Where the frame is seen from, as the shader takes it
///
/// The screen's axes in the galaxy, what a logical pixel covers a light year
/// in, the reach's sphere from the eye, where the eye stands in the cube, and
/// the volume's bounds and table.
#[derive(ShaderType, Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Frame {
    right: Vec3,
    /// Light years a logical pixel covers at a depth of one light year.
    per_pixel: f32,
    up: Vec3,
    /// The [`Table`]'s size less one.
    mask: u32,
    forward: Vec3,
    /// Light years the volume fades out over inside the reach.
    soft: f32,
    /// The reach's centre from the eye, light years.
    reach: Vec3,
    /// Its radius, light years; nought where the spyglass is not clearing.
    radius: f32,
    /// The eye from the cube's low corner, light years: where the table's
    /// places are counted from.
    root: Vec3,
    /// Which target this draw lays, as the bit a span marks it by.
    target: u32,
    /// The corners of the box every box stands in, from the eye.
    low: Vec3,
    _pad0: f32,
    high: Vec3,
    _pad1: f32,
    /// Half the frame, logical pixels.
    half: Vec2,
    _pad2: Vec2,
}

impl Frame {
    /// The frame seen by `orbit`, through a lens of `cot_half_fov` onto a
    /// `viewport` of logical pixels, clipped to `reach` where the spyglass
    /// clears: its centre and radius, light years.
    pub(crate) fn new(
        orbit: &OrbitCamera,
        cot_half_fov: f32,
        viewport: Vec2,
        reach: Option<(DVec3, f32)>,
    ) -> Frame {
        let (at, radius) = reach
            .map_or((Vec3::ZERO, 0.), |(centre, radius)| {
                ((centre - orbit.eye()).as_vec3(), radius)
            });
        Frame {
            right: orbit.right(),
            per_pixel: 2. / (cot_half_fov * viewport.y),
            up: orbit.up(),
            forward: orbit.forward(),
            soft: radius * SOFT,
            reach: at,
            radius,
            root: (orbit.eye() - DVec3::from(CellId::ROOT.min_ly())).as_vec3(),
            half: viewport * 0.5,
            ..Frame::default()
        }
    }
}

/// What lays the volume's light: one quad over the volume, and the shader
/// marches each pixel's ray through it
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct VolumeLight {
    #[uniform(0)]
    frame: Frame,
    #[storage(1, read_only)]
    boxes: Handle<ShaderBuffer>,
    /// The density this target lays.
    #[storage(2, read_only)]
    density: Handle<ShaderBuffer>,
    #[storage(3, read_only)]
    spans: Handle<ShaderBuffer>,
    #[storage(4, read_only)]
    influence: Handle<ShaderBuffer>,
    #[storage(5, read_only)]
    table: Handle<ShaderBuffer>,
}

impl Material for VolumeLight {
    fn vertex_shader() -> ShaderRef {
        "embedded://galos_map/map/paint/volume.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://galos_map/map/paint/volume.wgsl".into()
    }

    /// Light laid into the field and summed, as the splats are.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers =
            vec![layout.0.get_layout(&[
                Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            ])?];
        Ok(())
    }
}

/// One of the two volume draws: the let-through on [`VOLUME_LAYER`], and what
/// the filters exclude on [`VOLUME_DIMMED_LAYER`]
#[derive(Component)]
pub(crate) struct VolumeMark {
    dimmed: bool,
}

/// Something laid out flat for a storage buffer
trait Plain {
    fn put(&self, into: &mut Vec<u8>);
}

impl Plain for [f32; 4] {
    fn put(&self, into: &mut Vec<u8>) {
        for value in self {
            into.extend_from_slice(&value.to_ne_bytes());
        }
    }
}

impl Plain for [u32; 2] {
    fn put(&self, into: &mut Vec<u8>) {
        for value in self {
            into.extend_from_slice(&value.to_ne_bytes());
        }
    }
}

impl Plain for [u32; 4] {
    fn put(&self, into: &mut Vec<u8>) {
        for value in self {
            into.extend_from_slice(&value.to_ne_bytes());
        }
    }
}

impl Plain for u32 {
    fn put(&self, into: &mut Vec<u8>) {
        into.extend_from_slice(&self.to_ne_bytes());
    }
}

/// One storage buffer, and how many bytes it was made to hold
///
/// Written at that size every time, so the buffer on the GPU is written in
/// place and the bind groups made over it stay good. Outgrown, it is a new
/// buffer under a new handle, which the material is handed in the same
/// frame and so makes its bind group over afresh.
struct Slot {
    handle: Handle<ShaderBuffer>,
    capacity: usize,
}

/// The least a buffer holds: one element of the widest kind, so a binding
/// is never empty.
const LEAST: usize = 16;

impl Slot {
    fn new(buffers: &mut Assets<ShaderBuffer>) -> Slot {
        Slot {
            handle: buffers.add(ShaderBuffer::new(
                &[0; LEAST],
                RenderAssetUsages::default(),
            )),
            capacity: LEAST,
        }
    }

    fn write<T: Plain>(
        &mut self,
        buffers: &mut Assets<ShaderBuffer>,
        items: &[T],
    ) {
        let mut bytes = Vec::with_capacity(self.capacity);
        for item in items {
            item.put(&mut bytes);
        }
        if bytes.len() > self.capacity {
            self.capacity = bytes.len().next_power_of_two();
            bytes.resize(self.capacity, 0);
            self.handle = buffers
                .add(ShaderBuffer::new(&bytes, RenderAssetUsages::default()));
            return;
        }
        bytes.resize(self.capacity, 0);
        if let Some(mut buffer) = buffers.get_mut(&self.handle) {
            buffer.data = Some(bytes);
        }
    }
}

/// The volume's buffers
#[derive(Resource)]
pub(crate) struct Held {
    boxes: Slot,
    spans: Slot,
    influence: Slot,
    table: Slot,
    density: [Slot; 2],
}

/// The one quad a draw is: `x` and `y` which corner. The vertex shader lays
/// it over the volume.
fn quad() -> Mesh {
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[0f32, 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
    );
    mesh.insert_indices(bevy::mesh::Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

/// Put the two draws up, hidden until the glow lays a volume
fn spawn_volume(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut materials: ResMut<Assets<VolumeLight>>,
) {
    let held = Held {
        boxes: Slot::new(&mut buffers),
        spans: Slot::new(&mut buffers),
        influence: Slot::new(&mut buffers),
        table: Slot::new(&mut buffers),
        density: [Slot::new(&mut buffers), Slot::new(&mut buffers)],
    };
    let quad = meshes.add(quad());
    for (target, (dimmed, layer)) in
        [(false, VOLUME_LAYER), (true, VOLUME_DIMMED_LAYER)]
            .into_iter()
            .enumerate()
    {
        commands.spawn((
            Mesh3d(quad.clone()),
            MeshMaterial3d(
                materials.add(held.material(target, Frame::default())),
            ),
            RenderLayers::layer(layer),
            // Placed by the vertex shader; there is no bound to cull against.
            NoFrustumCulling,
            Transform::default(),
            Visibility::Hidden,
            VolumeMark { dimmed },
        ));
    }
    commands.insert_resource(held);
}

impl Held {
    fn material(&self, target: usize, frame: Frame) -> VolumeLight {
        VolumeLight {
            frame,
            boxes: self.boxes.handle.clone(),
            density: self.density[target].handle.clone(),
            spans: self.spans.handle.clone(),
            influence: self.influence.handle.clone(),
            table: self.table.handle.clone(),
        }
    }
}

/// What the glow writes the volume through
///
/// Each part optional, so a harness that stands up the glow without a
/// renderer lays no volume rather than failing to run.
#[derive(SystemParam)]
pub(crate) struct Volumes<'w, 's> {
    held: Option<ResMut<'w, Held>>,
    buffers: Option<ResMut<'w, Assets<ShaderBuffer>>>,
    materials: Option<ResMut<'w, Assets<VolumeLight>>>,
    draws: Query<
        'w,
        's,
        (
            &'static VolumeMark,
            &'static MeshMaterial3d<VolumeLight>,
            &'static mut Visibility,
        ),
    >,
}

impl Volumes<'_, '_> {
    /// Hand `built` to the shader, seen as `frame` says; a draw with no
    /// light to lay is hidden
    pub(crate) fn lay(&mut self, built: &Built, frame: Frame) {
        let Volumes { held, buffers, materials, draws } = self;
        let (Some(held), Some(buffers), Some(materials)) = (
            held.as_deref_mut(),
            buffers.as_deref_mut(),
            materials.as_deref_mut(),
        ) else {
            return;
        };
        if built.lights.contains(&true) {
            held.boxes.write(buffers, &built.boxes);
            held.spans.write(buffers, &built.spans);
            held.influence.write(buffers, &built.influence);
            held.table.write(buffers, &built.table);
        }
        let frame = Frame {
            mask: (built.table.len() as u32).saturating_sub(1),
            low: built.low,
            high: built.high,
            ..frame
        };
        for (mark, material, mut visibility) in draws.iter_mut() {
            let target = usize::from(mark.dimmed);
            let shown = built.lights[target];
            visibility.set_if_neq(match shown {
                true => Visibility::Visible,
                false => Visibility::Hidden,
            });
            if !shown {
                continue;
            }
            held.density[target].write(buffers, &built.density[target]);
            if let Some(mut light) = materials.get_mut(&material.0) {
                *light = held
                    .material(target, Frame { target: 1 << target, ..frame });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field a box's list gives at `at`, light years from the eye, as
    /// `volume.wgsl` works it out: the mean of the densities over the cells
    /// whose octant bit is set for where `at` stands in the box
    fn field(built: &Built, at: DVec3) -> f32 {
        let at = at.as_vec3();
        let region = built
            .boxes
            .iter()
            .position(|b| {
                let centre = Vec3::new(b[0], b[1], b[2]);
                (at - centre).abs().max_element() <= 0.5 * b[3]
            })
            .expect("a point inside the volume's boxes");
        let b = built.boxes[region];
        let centre = Vec3::new(b[0], b[1], b[2]);
        let octant = u32::from(at.x > centre.x)
            | u32::from(at.y > centre.y) << 1
            | u32::from(at.z > centre.z) << 2;
        let [first, count] = built.spans[region];
        let count = count & LISTED;
        let (mut sum, mut weight) = (0., 0.);
        for entry in &built.influence[first as usize..(first + count) as usize]
        {
            if (entry >> OCTANTS) >> octant & 1 == 0 {
                continue;
            }
            let k = (entry & CELL) as usize;
            let cell = built.boxes[k];
            let d = (Vec3::ONE
                - (at - Vec3::new(cell[0], cell[1], cell[2])).abs() / cell[3])
                .max(Vec3::ZERO);
            let w = d.x * d.y * d.z;
            sum += built.density[0][k][0] * w;
            weight += w;
        }
        if weight > 0. { sum / weight } else { 0. }
    }

    /// A level-`level` cell `(x, y, z)` along from one near the middle of
    /// the cube.
    fn cell(level: u8, x: u32, y: u32, z: u32) -> CellId {
        let base = 1u32 << (level - 1);
        CellId { level, x: base + x, y: base + y, z: base + z }
    }

    fn source(id: CellId, light: f32) -> Source {
        Source { id, lit: Vec3::splat(light), dimmed: Vec3::ZERO }
    }

    /// Where `id`'s middle stands, from an eye at the cube's origin.
    fn middle(id: CellId) -> DVec3 {
        DVec3::from(id.bounds().center())
    }

    /// The same light in every cell of a block is one density inside it,
    /// on the cells' pitch and between: no lattice
    #[test]
    fn an_even_crowd_is_flat() {
        let level = 10;
        let edge = CellId::edge_at(level);
        let sources: Vec<Source> = (0..6u32)
            .flat_map(|x| {
                (0..6u32).flat_map(move |y| (0..6u32).map(move |z| (x, y, z)))
            })
            .map(|(x, y, z)| source(cell(level, x, y, z), 7.))
            .collect();
        let built = Built::of(&sources, DVec3::ZERO);
        let density = 7. / edge.powi(3) as f32;
        // From the middle of the second cell to the middle of the fifth,
        // where every tent a point stands under is the crowd's own.
        let from = middle(cell(level, 1, 1, 1));
        for step in 0..=60 {
            let s = f64::from(step) / 60. * 3.;
            for dir in
                [DVec3::X, DVec3::new(1., 0.7, 0.3), DVec3::new(0.2, 1., 0.9)]
            {
                let at = from + dir * s * edge;
                let got = field(&built, at);
                assert!(
                    (got / density - 1.).abs() < 1e-4,
                    "{got} where the crowd is {density}, {s} cells along"
                );
            }
        }
    }

    /// An even crowd stays flat where its cells change size, and where they
    /// change by more than one
    #[test]
    fn flat_where_cells_change_size() {
        for finer in [1u8, 2] {
            let coarse = 9;
            let fine = coarse + finer;
            let k = 1u32 << finer;
            let edge = CellId::edge_at(coarse);
            let density = 3. / edge.powi(3) as f32;
            let mut sources = Vec::new();
            for x in 0..6u32 {
                for y in 0..4u32 {
                    for z in 0..4u32 {
                        let big = cell(coarse, x, y, z);
                        if x < 3 {
                            sources.push(source(big, 3.));
                            continue;
                        }
                        for child in 0..k * k * k {
                            let id = CellId {
                                level: fine,
                                x: big.x * k + child % k,
                                y: big.y * k + child / k % k,
                                z: big.z * k + child / (k * k),
                            };
                            let light = 3. / (k * k * k) as f32;
                            sources.push(source(id, light));
                        }
                    }
                }
            }
            let built = Built::of(&sources, DVec3::ZERO);
            // Across the seam at x = 3, well inside in y and z.
            let from = middle(cell(coarse, 1, 1, 1));
            for step in 0..=80 {
                let s = f64::from(step) / 80. * 3.;
                let at = from + DVec3::new(s, 0.37 + s * 0.1, 0.61) * edge;
                let got = field(&built, at);
                assert!(
                    (got / density - 1.).abs() < 1e-4,
                    "{got} where the crowd is {density}, {s} cells along, \
                     {finer} levels finer past the seam"
                );
            }
        }
    }

    /// Cells of three sizes and uneven light, with holes in it.
    fn uneven() -> Vec<Source> {
        let mut sources = Vec::new();
        let mut light = 1.;
        for x in 0..5u32 {
            for y in 0..3u32 {
                for z in 0..3u32 {
                    light = (light * 7.31) % 11. + 0.5;
                    let big = cell(8, x, y, z);
                    match (x + y * 2 + z) % 4 {
                        0 => sources.push(source(big, light)),
                        1 => {
                            for child in big.children().into_iter().step_by(2) {
                                sources.push(source(child, light));
                            }
                        }
                        2 => {
                            for child in big.children() {
                                for grand in
                                    child.children().into_iter().take(5)
                                {
                                    sources.push(source(grand, light * 0.3));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        sources
    }

    /// The field is one value either side of every face between two boxes,
    /// whatever the cells hold: every box's list holds every tent that
    /// reaches into it, so the boxes meet without a seam. And it falls to
    /// nothing across the ghosts past the crowd's edge.
    #[test]
    fn the_boxes_meet_without_a_seam() {
        let built = Built::of(&uneven(), DVec3::ZERO);

        // No box stands inside another, or the light inside both would be
        // counted twice along every ray through it.
        let found: Vec<(Vec3, f32)> = built
            .boxes
            .iter()
            .map(|b| (Vec3::new(b[0], b[1], b[2]), b[3]))
            .collect();
        for (i, (a, ea)) in found.iter().enumerate() {
            for (b, eb) in &found[i + 1..] {
                let apart = (*a - *b).abs();
                let reach = Vec3::splat(0.5 * (ea + eb));
                assert!(
                    apart.cmpge(reach - 1e-3).any(),
                    "two of the volume's boxes overlap"
                );
            }
        }

        // Either side of each box's faces, at points scattered over them.
        let mut checked = 0;
        for (centre, edge) in &found {
            for axis in 0..3 {
                for sign in [-1f32, 1.] {
                    for (u, v) in
                        [(0.13f32, -0.29f32), (-0.41, 0.07), (0.33, 0.44)]
                    {
                        let mut at = *centre;
                        at[axis] += sign * 0.5 * edge;
                        at[(axis + 1) % 3] += u * edge;
                        at[(axis + 2) % 3] += v * edge;
                        let mut off = Vec3::ZERO;
                        off[axis] = sign * 1e-3 * edge;
                        let (inside, outside) = (at - off, at + off);
                        let held = |p: Vec3| {
                            found.iter().any(|(c, e)| {
                                (p - *c).abs().max_element() < 0.5 * e
                            })
                        };
                        if !held(outside) {
                            // Past every box: the field there is nothing, so
                            // it must have come down to nothing here.
                            let here = field(&built, inside.as_dvec3());
                            assert!(
                                here.abs() < 1e-3 * densest(&built),
                                "the field ends on an edge: {here}"
                            );
                            continue;
                        }
                        let a = field(&built, inside.as_dvec3());
                        let b = field(&built, outside.as_dvec3());
                        assert!(
                            (a - b).abs() <= 1e-2 * densest(&built),
                            "a seam: {a} one side of a face, {b} the other"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 1000, "only {checked} faces between boxes checked");
    }

    fn densest(built: &Built) -> f32 {
        built.density[0].iter().map(|d| d[0]).fold(0., f32::max)
    }

    /// What `table` holds for `id`, looked up as `volume.wgsl`'s `probe` does.
    fn held_at(table: &[[u32; 4]], id: CellId) -> Option<u32> {
        let mask = table.len() as u32 - 1;
        let tagged = id.z | u32::from(id.level) << 24;
        let mut slot = scatter(id.x, id.y, tagged) & mask;
        loop {
            let held = table[slot as usize];
            if held[2] == Table::EMPTY {
                return None;
            }
            if held[..3] == [id.x, id.y, tagged] {
                return Some(held[3]);
            }
            slot = (slot + 1) & mask;
        }
    }

    /// Walking the table down from the root at any point ends on the box
    /// the point is in, or on a cell no box is in
    ///
    /// Which is what the march stands on: it finds the box a point is in by
    /// the deepest level the table holds there, and steps over an empty cell
    /// whole. A cell holding boxes left out of the table would hide them, and
    /// a box taken for an empty cell would lose its light.
    #[test]
    fn the_table_finds_the_box_a_point_is_in() {
        let built = Built::of(&uneven(), DVec3::ZERO);
        let boxes: Vec<(DVec3, f64)> = built
            .boxes
            .iter()
            .map(|b| {
                (
                    DVec3::new(b[0].into(), b[1].into(), b[2].into()),
                    f64::from(b[3]),
                )
            })
            .collect();
        // Off the grid by a fraction no power of two divides, so no point
        // stands on a face, where it is in both boxes or neither.
        let low = DVec3::from(cell(8, 0, 0, 0).bounds().min) - 1500.318;
        let span = DVec3::from(cell(8, 4, 2, 2).bounds().max) + 1500. - low;
        let (mut inside, mut outside) = (0, 0);
        for k in 0..20_000u32 {
            // A low-discrepancy scatter over the layout and a margin round it.
            let f = |n: u32, base: f64| {
                let (mut at, mut part, mut n) = (0., 1. / base, n + 1);
                while n > 0 {
                    at += f64::from(n % base as u32) * part;
                    n /= base as u32;
                    part /= base;
                }
                at
            };
            let point = low + span * DVec3::new(f(k, 2.), f(k, 3.), f(k, 5.));
            let mut level = 0;
            let mut found =
                held_at(&built.table, CellId::of_point(point.to_array(), 0))
                    .expect("the root holds the volume");
            while found == Table::HOLDS && level < MAX_LEVEL {
                let Some(deeper) = held_at(
                    &built.table,
                    CellId::of_point(point.to_array(), level + 1),
                ) else {
                    break;
                };
                level += 1;
                found = deeper;
            }
            let holding = boxes
                .iter()
                .position(|(c, e)| (point - *c).abs().max_element() < 0.5 * e);
            match found {
                Table::HOLDS => {
                    assert_eq!(
                        holding, None,
                        "a box at {point} was taken for empty space"
                    );
                    outside += 1;
                }
                region => {
                    assert_eq!(
                        holding,
                        Some(region as usize),
                        "the wrong box at {point}"
                    );
                    inside += 1;
                }
            }
        }
        assert!(
            inside > 1000 && outside > 1000,
            "{inside} inside, {outside} out"
        );
    }

    /// A share of nothing under the band, all of it over, and the same at
    /// either end however the camera comes at it
    #[test]
    fn the_share_hands_over_across_the_band() {
        assert_eq!(share(FROM_PX * 0.5), 0.);
        assert_eq!(share(FROM_PX), 0.);
        assert_eq!(share(FULL_PX), 1.);
        assert_eq!(share(f32::INFINITY), 1.);
        assert_eq!(share(f32::NAN), 0.);
        let mut last = 0.;
        for step in 0..=100 {
            let px = FROM_PX + (FULL_PX - FROM_PX) * step as f32 / 100.;
            let now = share(px);
            assert!(now >= last, "the share turns back at {px} px");
            last = now;
        }
    }
}
