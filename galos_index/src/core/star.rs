//! The one byte a system carries about the star a ship arrives at.
//!
//! Here, below everything, because both the build's [`crate::system::System`]
//! and the payload's [`crate::tree::cell::CellSystem`] carry it.

use serde::{Deserialize, Serialize};

/// What kind of star a system arrives at, in one byte
///
/// **The fact a fuel-aware route is built on, sized to be carried rather
/// than looked up.** A router asks it of every system it expands, which
/// rules out anything with a hash in it: the classed systems are some
/// ninety-five million, and a resident map over them is gigabytes. A byte
/// is what a cell's payload can hold beside a position the router is
/// faulting anyway.
///
/// The kinds are the families the game distinguishes and not the spectrum:
/// what a route needs to know is whether a ship can refuel, whether it can
/// supercharge, and what to call the thing. A subclass and a luminosity say
/// nothing to either question.
///
/// [`Self::Unknown`] is a real answer and the commonest one. Most of the
/// galaxy has never been scanned, and "nothing has looked" has to be told
/// apart from "nothing to scoop" — a route across unexplored space would
/// otherwise read as a route that strands.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum StarKind {
    /// Nothing has said, which is most of the sky
    #[default]
    Unknown = 0,
    /// The main sequence, which is what a fuel scoop lives on
    O = 1,
    B = 2,
    A = 3,
    F = 4,
    G = 5,
    K = 6,
    M = 7,
    /// Too cool to have started fusing: L, T and Y
    BrownDwarf = 8,
    /// What is left when the hydrogen went, and half again on a jump
    WhiteDwarf = 9,
    /// The same, and four times over: class N
    Neutron = 10,
    /// Class H, and the supermassive one at the centre
    BlackHole = 11,
    /// Carbon and S-type: an envelope of the wrong element
    Carbon = 12,
    /// Wolf-Rayet, which is a core with its envelope blown off
    WolfRayet = 13,
    /// Still forming: T Tauri and Herbig Ae/Be
    Forming = 14,
    /// Something the galaxy holds that none of the above names
    Other = 15,
}

impl StarKind {
    /// How many kinds there are, [`Self::Unknown`] included: one past the
    /// largest [`Self::code`]
    pub const COUNT: usize = 16;

    /// What the class the journals and the dumps state comes to
    ///
    /// Matched on the whole class rather than its first letter, because the
    /// sky has classes that begin with a scoopable letter and hold nothing
    /// to scoop: `MS` and `S` are S-type stars and `AeBe` is a Herbig star,
    /// while `M_RedGiant` and `K_OrangeGiant` are the same stars grown.
    pub fn of(primary_star_class: &str) -> StarKind {
        let mut letters = primary_star_class.chars();
        let Some(first) = letters.next() else {
            return StarKind::Unknown;
        };
        let sort = letters.next();
        // The letter alone, or the letter and what kind of one it is.
        let plain = matches!(sort, None | Some('_'));
        match (first, plain) {
            ('O', true) => StarKind::O,
            ('B', true) => StarKind::B,
            ('A', true) => StarKind::A,
            ('F', true) => StarKind::F,
            ('G', true) => StarKind::G,
            ('K', true) => StarKind::K,
            ('M', true) => StarKind::M,
            ('L' | 'T' | 'Y', true) => StarKind::BrownDwarf,
            ('N', true) => StarKind::Neutron,
            ('H', _) => StarKind::BlackHole,
            ('D', _) => StarKind::WhiteDwarf,
            ('W', _) => StarKind::WolfRayet,
            ('C' | 'S', _) | ('M', false) => StarKind::Carbon,
            ('T', false) => StarKind::Forming,
            ('A', false) => StarKind::Forming,
            _ => StarKind::Other,
        }
    }

    /// Whether a ship can refuel here
    ///
    /// A fuel scoop takes hydrogen out of a star's corona, and a star either
    /// has hydrogen to give or it does not: the main sequence classes **K,
    /// G, B, F, O, A and M** do, and nothing else in the sky does. A brown
    /// dwarf is too cool to have started fusing, a white dwarf and a neutron
    /// star are what is left after the hydrogen went, a carbon star's
    /// envelope is the wrong element, and a black hole gives nothing at all.
    /// Which is why a route that cannot be refuelled is not a slower route
    /// but a stranded ship.
    ///
    /// **Temperature cannot answer it**: the map's six log-spaced buckets put
    /// M dwarfs, brown dwarfs and black holes in the same bucket and O stars
    /// in with neutron stars, so the class itself is the only honest source.
    ///
    /// [`Self::Unknown`] answers false and the caller has to tell the two
    /// apart itself — see the type's own note.
    pub fn scoopable(&self) -> bool {
        matches!(
            self,
            StarKind::O
                | StarKind::B
                | StarKind::A
                | StarKind::F
                | StarKind::G
                | StarKind::K
                | StarKind::M
        )
    }

    /// What it is called, for a reader rather than for a router
    ///
    /// [`None`] where nothing has said, so a row can leave the column empty
    /// rather than print a guess.
    pub fn named(&self) -> Option<&'static str> {
        Some(match self {
            StarKind::Unknown => return None,
            StarKind::O => "class O",
            StarKind::B => "class B",
            StarKind::A => "class A",
            StarKind::F => "class F",
            StarKind::G => "class G",
            StarKind::K => "class K",
            StarKind::M => "class M",
            StarKind::BrownDwarf => "brown dwarf",
            StarKind::WhiteDwarf => "white dwarf",
            StarKind::Neutron => "neutron star",
            StarKind::BlackHole => "black hole",
            StarKind::Carbon => "carbon star",
            StarKind::WolfRayet => "Wolf-Rayet",
            StarKind::Forming => "forming star",
            StarKind::Other => "unusual star",
        })
    }

    /// The byte a payload carries it as.
    pub fn code(&self) -> u8 {
        *self as u8
    }

    /// And back, anything unrecognised reading as nothing having been said:
    /// a byte from a build that knew kinds this one does not is a kind this
    /// one cannot claim anything about.
    pub fn from_code(code: u8) -> StarKind {
        match code {
            1 => StarKind::O,
            2 => StarKind::B,
            3 => StarKind::A,
            4 => StarKind::F,
            5 => StarKind::G,
            6 => StarKind::K,
            7 => StarKind::M,
            8 => StarKind::BrownDwarf,
            9 => StarKind::WhiteDwarf,
            10 => StarKind::Neutron,
            11 => StarKind::BlackHole,
            12 => StarKind::Carbon,
            13 => StarKind::WolfRayet,
            14 => StarKind::Forming,
            15 => StarKind::Other,
            _ => StarKind::Unknown,
        }
    }
}
