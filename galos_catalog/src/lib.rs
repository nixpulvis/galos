//! Star catalogs measured from Earth, in the vocabulary the workspace speaks.
//!
//! A catalog is a source of stars exactly as the Elite dataset is a source of
//! systems, and the two meet where every other source does: a position, an
//! absolute magnitude and a temperature. That makes this a peer of
//! `galos_db`'s bake rather than of the database — it reads its own data and
//! hands back [`Star`]s, and knows nothing about the tree, the renderer or
//! Postgres.
//!
//! What it owns is everything between a foreign file and that vocabulary.
//!
//! - **Parsing.** [`hyg`] today; another survey is another module beside it.
//! - **Units.** Catalogs are written in parsecs and the workspace stands in
//!   light years, so the conversion happens here, once, at the read. A
//!   [`Star`] is always in light years.
//! - **Frames.** A catalog's axes are the sky's as seen from Earth. [`frame`]
//!   is where those are turned into anything else.
//!
//! [`Star`] is deliberately richer than `galos_index::System`. A catalog row
//! carries a name, a measured apparent magnitude, a colour index and a
//! spectral type, and the reason to read a real catalog at all is to check
//! claims against those. `System` is the lossy projection that survives into
//! the tree, and producing it is this crate's output rather than its type —
//! see [`index`], behind the `index` feature.
//!
//! # More than one survey
//!
//! [`Star`], [`Unplaced`], [`Catalog`] and [`Skipped`] live here rather than in
//! [`hyg`], because none of them is about HYG. They are what *any* survey
//! produces: stars with a place, stars with only a bearing, and rows that could
//! not be read. A second source is a new module with its own `read`, returning
//! the same [`Catalog`], and nothing downstream changes.
//!
//! What a second source does need is a [`Source`], because two catalogs number
//! their rows from one and would otherwise collide — the same problem Elite's
//! addresses have with a catalog's ids, and solved the same way, by giving each
//! its own range rather than hoping. A [`Star`] therefore knows where it came
//! from, and [`Star::to_system`] folds that into the id it hands the index.

pub mod check;
pub mod compare;
pub mod frame;
pub mod hyg;
#[cfg(feature = "index")]
pub mod index;

use galos_photometry::{EYE_LIMIT, class_light, color_index_to_temperature};

/// Which survey a star came from.
///
/// Two catalogs both number their rows from one, so an id alone says nothing
/// about which star it is once more than one has been read. A source is that
/// missing half: a name for a person and a namespace for an id.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// What to call it in a report.
    pub name: &'static str,
    /// Its range among catalogs, folded into the ids handed to the index.
    ///
    /// Small and hand-assigned rather than hashed from the name, so the id a
    /// star gets is stable across renames and reproducible from the source
    /// alone. Six bits, which is sixty-four surveys — well past any plausible
    /// number of them, and cheap because the id it shares a word with has
    /// fifty-six left over.
    pub namespace: u8,
}

/// The HYG catalog: Hipparcos, Yale Bright Star and Gliese, merged.
pub const HYG: Source = Source { name: "HYG", namespace: 1 };

/// How many bits of an id a [`Source::namespace`] occupies, from bit 56.
pub const NAMESPACE_BITS: u32 = 6;

/// The largest id a catalog row may carry before it collides with a namespace.
///
/// Fifty-six bits. HYG numbers its rows in the hundreds of thousands and is in
/// no danger; a survey whose identifiers are larger than this — Gaia's
/// `source_id` packs a sky position into sixty-three bits — has to renumber on
/// the way in rather than pass its own through.
pub const MAX_ID: u64 = (1 << (64 - 8 - NAMESPACE_BITS)) - 1;

/// What a read passed over, and why.
///
/// Returned beside the stars rather than logged, so a caller that cares can
/// assert on it and one that does not can ignore it.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Skipped {
    /// Rows at distance zero: Sol, and anything else sitting on the observer.
    pub at_the_origin: usize,
    /// Rows carrying a no-parallax sentinel or no usable distance.
    pub no_parallax: usize,
    /// Rows whose required columns would not parse.
    pub unreadable: usize,
    /// How many of the dropped rows are naked-eye stars.
    ///
    /// The figure that says whether a hole matters. Ten thousand discarded
    /// twelfth-magnitude rows change no picture; a hundred discarded
    /// fifth-magnitude ones change every one, and both look the same in
    /// [`total`](Self::total).
    pub naked_eye: usize,
    /// The apparent magnitude of the brightest row dropped, if any were.
    pub brightest: Option<f64>,
}

impl Skipped {
    /// How many rows were dropped in total.
    pub fn total(&self) -> usize {
        self.at_the_origin + self.no_parallax + self.unreadable
    }

    /// Note a dropped row's magnitude against the naked-eye tally.
    pub(crate) fn note(&mut self, magnitude: f64) {
        if magnitude <= EYE_LIMIT {
            self.naked_eye += 1;
        }
        self.brightest = Some(match self.brightest {
            Some(brightest) => brightest.min(magnitude),
            None => magnitude,
        });
    }
}

/// A star a catalog locates on the sky but not in space.
///
/// A measured bearing and a measured brightness, and no distance. Everything
/// here is real; what is absent is absent rather than guessed.
#[derive(Clone, Debug, PartialEq)]
pub struct Unplaced {
    /// The catalog's own identifier.
    pub id: u64,
    /// A proper name or designation, where the row carries one.
    pub name: Option<String>,
    /// The unit direction **from Sol**, in the catalog's own frame.
    ///
    /// From Sol this is exactly where the star is on the sky. From anywhere
    /// else it says only which line the star lies along, and how far up that
    /// line is what the catalog does not know.
    pub direction: [f64; 3],
    /// Apparent visual magnitude, as measured from Earth.
    pub apparent_magnitude: f64,
    /// The spectral type as the catalog spells it.
    pub spectral_type: Option<String>,
}

/// What one read of a catalog produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    /// Which survey this came from.
    pub source: Source,
    /// The stars with a place in space.
    pub stars: Vec<Star>,
    /// The stars with a bearing but no distance — the hole, enumerated.
    pub unplaced: Vec<Unplaced>,
    /// What was dropped outright, and how bright.
    pub skipped: Skipped,
}

impl Catalog {
    /// An empty catalog from a given survey.
    pub fn new(source: Source) -> Catalog {
        Catalog {
            source,
            stars: Vec::new(),
            unplaced: Vec::new(),
            skipped: Skipped::default(),
        }
    }
}

/// One star as a catalog records it.
///
/// Positions are light years in the catalog's own frame, converted at the read
/// so nothing downstream has to remember which unit a given survey was written
/// in. Everything optional is optional because real catalogs have holes in
/// them, and a star with no colour index and no spectral type still has a
/// place and a brightness.
#[derive(Clone, Debug, PartialEq)]
pub struct Star {
    /// Which survey it came from.
    pub source: Source,
    /// The catalog's own identifier, unique within that catalog but not
    /// between two of them — see [`Source::namespace`].
    pub id: u64,
    /// The Hipparcos number, where the star has one.
    pub hip: Option<u32>,
    /// A proper name — "Sirius", "Betelgeuse" — for the few hundred that have
    /// one.
    pub name: Option<String>,
    /// Where it sits, light years, in the catalog's frame.
    pub position: [f64; 3],
    /// How far away, light years. Held rather than recomputed because a
    /// catalog's distance is a measurement — a parallax — and the position is
    /// derived from it, not the other way about.
    pub distance: f64,
    /// Apparent visual magnitude as measured from Earth.
    ///
    /// The column nothing in the workspace can predict and everything can be
    /// checked against: this is the number [`galos_photometry`]'s distance
    /// modulus has to reproduce from the absolute magnitude and the distance.
    pub apparent_magnitude: f64,
    /// Absolute visual magnitude.
    pub absolute_magnitude: f64,
    /// `B-V` colour index, where measured.
    pub color_index: Option<f64>,
    /// The spectral type as the catalog spells it — `A0m...`, `K2IIIp`, `F5`.
    pub spectral_type: Option<String>,
}

impl Star {
    /// The star's effective temperature, kelvin, by the best route the row
    /// allows.
    ///
    /// A measured colour index first, since it is a measurement of this star;
    /// then the spectral type through [`class_light`], which is a typical
    /// figure for its family; then the default that stands in for a row with
    /// neither. The same shape of fallback the bake uses, for the same reason:
    /// a catalog is holey and a star with no colour still has to be given a
    /// tint.
    ///
    /// `class_light` reads Elite's class tokens, which are the leading letters
    /// of a spectral type, and a catalog's `K2IIIp` leads with the same letter
    /// a `K` does. The luminosity class is dropped in the process, so a giant
    /// is read as a dwarf of its letter — which is why a colour index, when
    /// there is one, is always the better answer.
    pub fn temperature(&self) -> f64 {
        match self.color_index {
            Some(bv) => color_index_to_temperature(bv),
            None => class_light(self.spectral_type.as_deref().unwrap_or(""))
                .temperature,
        }
    }
}
