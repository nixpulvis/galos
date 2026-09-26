//! A system's name, in the one spelling the whole of galos uses.
//!
//! Elite's sources disagree about case. A journal writes `Sol`, Spansh's
//! dump writes `Sol`, EDDN carries whatever the commander's client sent, and
//! `galos_db` has always written `UPPER($2)` — so the database's spelling is
//! upper case and every reader that compares against it had to say so. Four
//! places did: `SystemReport::named` uppercased on every read,
//! `Names::find` lowercased *both sides of every comparison* over a hundred
//! and thirty-one million entries, `names_exactly` and `address` folded case
//! per entry, and the SQL did it again in the server.
//!
//! [`SystemName`] is the invariant instead of the convention: a value of it
//! is upper case because there is no way to make one that is not. The fold
//! happens once, where a name enters the program — a parse, a row, a decode
//! — and never again. Two names are then compared as bytes, sorted as
//! bytes, and hashed as bytes, which is what makes a sorted names index and
//! an `O(log N)` lookup possible at all: a case-insensitive comparison has
//! no order to binary-search.
//!
//! ## Why it is free where it matters
//!
//! Every name the game produces is ASCII — letters, digits, spaces, `-` and
//! `+`. So the fold is [`str::make_ascii_uppercase`] **in place** on a
//! `String` already owned, and a name that is already upper case, which is
//! every name read back out of a published directory or a `systems` row, is
//! not touched at all. Decoding the names table at 131 M entries allocates
//! nothing for this. The non-ASCII road is kept correct rather than fast:
//! `to_uppercase` allocates, and a galaxy of them would be a galaxy nobody
//! has.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;

/// A system's name, upper case by construction.
///
/// Serializes as the string it is, so a directory or a row written before
/// this type is read by it unchanged — and read *upper*, the fold being on
/// the way in.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemName(String);

impl SystemName {
    /// The name of a system, however the source spelled it.
    ///
    /// The one way in. `impl Into<String>` rather than `&str` so a caller
    /// that already owns the string — a deserializer, a database row —
    /// hands it over and the fold is in place.
    pub fn new(name: impl Into<String>) -> SystemName {
        let mut name = name.into();
        match name.is_ascii() {
            // In place, and not even that where it is upper already:
            // `make_ascii_uppercase` writes only the bytes that change.
            true => name.make_ascii_uppercase(),
            false => name = name.to_uppercase(),
        }
        SystemName(name)
    }

    /// The name as a string, for anything that takes one.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name, giving up the type with the string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<&str> for SystemName {
    fn from(name: &str) -> SystemName {
        SystemName::new(name)
    }
}

impl From<String> for SystemName {
    fn from(name: String) -> SystemName {
        SystemName::new(name)
    }
}

impl From<SystemName> for String {
    fn from(name: SystemName) -> String {
        name.0
    }
}

/// So a name is read wherever a `&str` is wanted — `format!`, `contains`,
/// `len`, a path, a query parameter — without a conversion or a copy.
impl Deref for SystemName {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SystemName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for SystemName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SystemName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Against a plain string, for a caller comparing with something it has
/// folded itself. Exact: two `SystemName`s are both upper, so nothing here
/// needs to know about case.
impl PartialEq<str> for SystemName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for SystemName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<SystemName> for str {
    fn eq(&self, other: &SystemName) -> bool {
        self == other.0
    }
}

impl Serialize for SystemName {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        out.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SystemName {
    /// Folded on the way in, so a table written before this type reads as
    /// one spelling rather than two.
    fn deserialize<D: Deserializer<'de>>(
        de: D,
    ) -> Result<SystemName, D::Error> {
        Ok(SystemName::new(String::deserialize(de)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// There is no way to hold a name that is not upper case
    ///
    /// The whole of the type. Stated as a test because every reader that
    /// stopped folding case is now relying on it: a mixed-case name in the
    /// table would make `Names::address` miss a system the map can draw.
    #[test]
    fn a_name_is_upper_however_it_arrived() {
        for spelled in ["Sol", "sol", "SOL", "sOl"] {
            assert_eq!(SystemName::new(spelled).as_str(), "SOL");
        }
        assert_eq!(
            SystemName::new("Col 285 Sector wu-e c12-3").as_str(),
            "COL 285 SECTOR WU-E C12-3",
        );
        // Read back out of a table an older build wrote, which is the case
        // the deserializer has to cover.
        let read: SystemName = rmp_serde::from_slice(
            &rmp_serde::to_vec("Shinrarta Dezhra").unwrap(),
        )
        .unwrap();
        assert_eq!(read.as_str(), "SHINRARTA DEZHRA");
    }

    /// A name serializes as the string it is
    ///
    /// The names table, the body files and the `populated` table are all
    /// MessagePack with no version of their own, so the encoding cannot
    /// change with the type: a directory written by this build has to read
    /// in one written before it, and the other way round.
    #[test]
    fn a_name_is_a_string_on_disk() {
        let name = SystemName::new("SOL");
        assert_eq!(
            rmp_serde::to_vec(&name).unwrap(),
            rmp_serde::to_vec("SOL").unwrap(),
            "the type changed the bytes",
        );
    }

    /// Non-ASCII names fold correctly, even though the game has none
    #[test]
    fn a_name_outside_ascii_is_still_folded() {
        assert_eq!(SystemName::new("Straße").as_str(), "STRASSE");
    }
}
