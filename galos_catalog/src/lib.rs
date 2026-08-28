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
//! - **Parsing.** [`hyg`] today; another format is another module beside it.
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

pub mod frame;
pub mod hyg;
#[cfg(feature = "index")]
pub mod index;

use galos_photometry::{class_light, color_index_to_temperature};

/// One star as a catalog records it.
///
/// Positions are light years in the catalog's own frame, converted at the read
/// so nothing downstream has to remember which unit a given survey was written
/// in. Everything optional is optional because real catalogs have holes in
/// them, and a star with no colour index and no spectral type still has a
/// place and a brightness.
#[derive(Clone, Debug, PartialEq)]
pub struct Star {
    /// The catalog's own identifier, unique within that catalog.
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
