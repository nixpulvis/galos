//! The fixed-width records the whole format is built from.
//!
//! Three of them, and they are what every layer above names. A [`System`]
//! is one system as the build reads it — sixty-four bytes, `repr(C)`, which
//! is also what a resume point holds a galaxy of. A [`Point`] is the same
//! system as a cell's payload carries it, which is what the client draws and
//! the router measures. A [`StarKind`] is the one byte both carry about the
//! star a ship arrives at, and [`Boost`] is what that star can supercharge a
//! drive by.
//!
//! Here, below everything, because everything uses them: the codecs, the
//! build, the walks and the resume point. They carry no behaviour past their
//! own encoding and the few facts a star's class answers.

use crate::core::aggregate::temp_bucket;
use crate::core::codec::{Decode, Encode, FixedCodec, record};
use serde::{Deserialize, Serialize};

/// One system as the build reads it: where it is, the photometry the ordering
/// and the glow need, and when it was last updated.
///
/// Absolute magnitude and temperature are the finished figures from the
/// photometry fallback chain (scanned stars summed, else the primary's class,
/// else a default), not anything the build works out. `age_bucket` is the
/// Recency axis the caller has already binned; `updated_at` is the same fact
/// unbinned, Unix seconds, so the Recency filter has something finer than a
/// day to test. The caller bins one from the other off one reading.
///
/// The record is written to disk as its own bytes, so it is `repr(C)`, fifty
/// six bytes, and padding-free; see [`crate::format::checkpoint`].
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct System {
    pub id64: u64,
    pub position: [f64; 3],
    pub absolute_magnitude: f64,
    pub temperature: f64,
    pub age_bucket: u32,
    pub updated_at: u32,
    /// What kind of star a ship arrives at
    ///
    /// Carried through the build so the payload can hold it: whether a ship
    /// refuels and whether it supercharges are this one byte, and the
    /// router reads it per expansion. See [`StarKind`].
    pub kind: StarKind,
}

/// The record width the resume point's format is written against. A field
/// added here without the format being told would read a checkpoint of one
/// galaxy back as another, so it fails the build instead.
///
/// Sixty-four with the star kind on it, where it was fifty-six: the byte
/// did not fit the `u32` pair's tail and took a word of its own. A resume
/// point written at the old width would be read as another galaxy, so
/// [`crate::format::checkpoint`]'s `VERSION` moved with it and a stale one is
/// refused rather than misread.
const _: () = assert!(std::mem::size_of::<System>() == 64);
const _: () = assert!(std::mem::align_of::<System>() == 8);

/// One system as the payload carries it: its id, its exact position, the
/// two photometric fields, and when it was last updated.
///
/// Position is three `f64` in light years, the system's own galactic
/// coordinates carried through unchanged, so a system is drawn exactly where
/// it sits however coarse the cell that owns it. The magnitude is the
/// system's combined absolute magnitude, carried at the `f32` the catalogue
/// holds it at, which its flux and the ordering are read from, and the
/// temperature bucket is the blackbody tint, already binned so the client
/// needs no per-star join.
///
/// `updated_at` is Unix seconds, and the one field here that is not about
/// where a system is or what it looks like. It is what the Recency filter
/// asks: which systems have been heard from lately. A cell's aggregate
/// answers that at a distance, counting systems per age bucket, but a bucket
/// is a day at its finest and the filter's shortest span is a minute, so the
/// per-system answer has to come from here. Four bytes on a record of
/// thirty-seven,
/// and the only table on the client's side of the wire that already rewrites
/// per system rather than per chunk: the cell a report moves is a file of tens
/// of kilobytes, where the names table's chunk is three megabytes and would go
/// dirty for every system reported.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Point {
    pub id64: u64,
    pub pos: [f64; 3],
    pub magnitude: f32,
    pub temp_bucket: u8,
    pub updated_at: u32,
    /// What kind of star a ship arrives at
    ///
    /// **Here because the router reads it per expansion.** Whether a ship
    /// can refuel and whether it can supercharge are both this one byte,
    /// and a route over the galaxy asks it of every system it reaches — so
    /// it rides beside the position, which that same loop has already
    /// faulted, rather than in a table of ninety-five million rows. See
    /// [`crate::core::record::StarKind`].
    pub kind: StarKind,
}

impl Point {
    /// A system packed into a payload point, its position carried through at
    /// full precision. The one place a record becomes a point, so the
    /// temperature bucketing lives here rather than at each caller that emits a
    /// payload.
    pub fn new(
        id64: u64,
        position: [f64; 3],
        magnitude: f64,
        temperature: f64,
        updated_at: u32,
        kind: StarKind,
    ) -> Point {
        Point {
            id64,
            pos: position,
            magnitude: magnitude as f32,
            temp_bucket: temp_bucket(temperature) as u8,
            updated_at,
            kind,
        }
    }
}

record! {
    Point {
        id64: u64,
        pos: [f64; 3],
        magnitude: f32,
        temp_bucket: u8,
        updated_at: u32,
        kind: StarKind,
    }
}

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
    /// G, B, F, O, A and M** do, and nothing else in the sky does. Which is
    /// why a route that cannot be refuelled is not a slower route but a
    /// stranded ship.
    ///
    /// [`Self::Unknown`] answers false and the caller has to tell the two
    /// apart itself — see the type's own note.
    pub fn scoops(&self) -> bool {
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

    /// What it can supercharge a drive on, where it can
    ///
    /// The same answer [`Boost::of`] reads off the class string, off the
    /// byte instead: a cone is a white dwarf's or a neutron star's.
    pub fn boost(&self) -> Option<Boost> {
        match self {
            StarKind::WhiteDwarf => Some(Boost::WhiteDwarf),
            StarKind::Neutron => Some(Boost::Neutron),
            _ => None,
        }
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

impl Encode for StarKind {
    fn encode(&self, out: &mut Vec<u8>) {
        Encode::encode(&self.code(), out);
    }
}

impl Decode for StarKind {
    fn decode(cur: &mut &[u8]) -> Option<StarKind> {
        Some(StarKind::from_code(<u8 as Decode>::decode(cur)?))
    }
}

impl FixedCodec for StarKind {
    const LEN: usize = 1;
}

/// What a system's arrival star can supercharge a frame shift drive by
///
/// Flying the jet cone of a neutron star or a white dwarf in supercruise, with
/// a fuel scoop, charges the drive for one jump: four times the range off a
/// neutron star, half again off a white dwarf, and more of both off a drive
/// built for it. The charge is held until a jump spends it, so what it is
/// worth is a fact about the system a ship is standing in and not about how it
/// got there — which is what lets the router read it as a property of a place.
///
/// Which of the two, rather than the multiplier: what a boost is worth depends
/// on the drive fitted, and the table is about the sky. See
/// `galos_map::systems::route::graph::Drive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Boost {
    /// A white dwarf: half again, and a much larger exclusion zone to be
    /// caught out by.
    WhiteDwarf,
    /// A neutron star: four times over, which is what a neutron highway is.
    Neutron,
}

impl Boost {
    /// What the star of class `primary_star_class` can supercharge, if it can
    ///
    /// The arrival star's class, which is the one that matters: a ship drops in
    /// at the main star and can reach its jet cone without crossing the system.
    /// A neutron star is class `N`; every white dwarf class begins with `D`
    /// (`DA`, `DB`, `DC` and their variants). Nothing else has a jet cone to
    /// fly — a black hole is class `H` and gives nothing, whatever it looks
    /// like it should.
    pub fn of(primary_star_class: &str) -> Option<Boost> {
        match primary_star_class {
            "N" => Some(Boost::Neutron),
            class if class.starts_with('D') => Some(Boost::WhiteDwarf),
            _ => None,
        }
    }

    /// What the star is called, for a reader rather than for a router
    ///
    /// The class is all this table keeps of a star — the index publishes
    /// what can supercharge and on what, not a spectrum — so it is the one
    /// thing a client can say about a star's kind without fetching the
    /// system's bodies.
    pub fn named(&self) -> &'static str {
        match self {
            Boost::Neutron => "neutron star",
            Boost::WhiteDwarf => "white dwarf",
        }
    }
}

/// Whether a ship can refuel at a star of this class
///
/// A fuel scoop takes hydrogen out of a star's corona, and a star either has
/// hydrogen to give or it does not: the main sequence classes **K, G, B, F,
/// O, A and M** do, and nothing else in the sky does. A brown dwarf is too
/// cool to have started fusing, a white dwarf and a neutron star are what is
/// left after the hydrogen went, a carbon star's envelope is the wrong
/// element, and a black hole gives nothing at all.
///
/// Which is why a route that cannot be refuelled is not a slower route but a
/// stranded ship, and the reason this is a fact worth publishing rather than
/// guessing. **Temperature cannot answer it**: the map's six log-spaced
/// buckets put M dwarfs, brown dwarfs and black holes in the same bucket and
/// O stars in with neutron stars, so the class itself is the only honest
/// source.
///
/// Matched on the whole class and not on its first letter, because the sky
/// has classes that begin with a scoopable letter and are not: `MS` and `S`
/// are S-type stars, which share no hydrogen envelope with an `M` dwarf, and
/// `AeBe` is a Herbig star rather than an `A`. The giants and supergiants
/// *are* the same star grown — `M_RedGiant`, `K_OrangeGiant`,
/// `A_BlueWhiteSuperGiant` — and the game scoops them, so a class is
/// scoopable when it is one of the seven letters exactly or that letter
/// followed by what kind of one it is.
pub fn scoopable(primary_star_class: &str) -> bool {
    StarKind::of(primary_star_class).scoops()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: u64, mag: f32) -> Point {
        Point {
            id64: id,
            pos: [10.5, -40000.25, 65535.0],
            magnitude: mag,
            temp_bucket: 3,
            updated_at: 1_757_260_000,
            kind: StarKind::G,
        }
    }

    /// A system survives the round trip through its bytes exactly, every
    /// field, the magnitude included.
    #[test]
    fn a_point_round_trips() {
        for mag in [-6.0, -1.5, 0.0, 4.83, 4.831_234_5, 15.0] {
            let p = point(42, mag);
            let mut buf = Vec::new();
            p.encode(&mut buf);
            assert_eq!(buf.len(), Point::LEN);
            let mut cur = &buf[..];
            let back = Point::decode(&mut cur).unwrap();
            assert_eq!(back.id64, p.id64);
            assert_eq!(back.pos, p.pos);
            assert_eq!(back.temp_bucket, p.temp_bucket);
            assert_eq!(back.updated_at, p.updated_at);
            assert_eq!(back.magnitude, p.magnitude);
        }
    }
}
