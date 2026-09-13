//! One system as `systems.json` gives it.

use crate::class_of;
use chrono::{DateTime, Utc};
use elite_journal::body::StarClass;
use elite_journal::system::Coordinate;
use serde::Deserialize;

/// A system in the brief dump: what it is called, where it is, what is at the
/// middle of it, and when anything was last heard about it.
///
/// The place is [`elite_journal::system::Coordinate`], as every other
/// source in the ecosystem carries one. The dump's own field names are
/// kept — `id64`, `coords`, `mainStar`; [`System::class`] turns the last
/// into the game's vocabulary.
///
/// Only the fields an index is built from are read. The schema declares
/// `additionalProperties: false`, so a field this does not want — such as
/// `needsPermit` — is simply passed over.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct System {
    /// The game's own 64-bit address, which every other source keys by too.
    ///
    /// Unsigned as the dump writes it and as the schema says (`minimum: 0`);
    /// `elite_journal` and `galos_db` carry the same number as an `i64`.
    pub id64: u64,
    pub name: String,
    pub coords: Coordinate,
    /// The class of the main star, as the dump's prose: `"M (Red dwarf)
    /// Star"`. Absent where nobody has ever looked at the middle of the
    /// system.
    #[serde(rename = "mainStar")]
    pub main_star: Option<String>,
    /// When the system was last updated, in UTC.
    #[serde(rename = "updateTime")]
    pub update_time: DateTime<Utc>,
}

impl System {
    /// What is at the middle of this system, in the game's vocabulary, or
    /// [`None`] where there is no star there or nobody has looked.
    ///
    /// See [`crate::star`] for the translation.
    pub fn class(&self) -> Option<StarClass> {
        class_of(self.main_star.as_deref()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line out of the real file parses to what it says.
    #[test]
    fn a_line_of_the_dump_reads() {
        let line = r#"{"id64":688319,"name":"Cygni X-3","mainStar":"Black Hole","coords":{"x":-36417.71875,"y":452.0625,"z":6522.71875},"updateTime":"2026-07-27T13:33:12Z"}"#;
        let system: System = serde_json::from_str(line).expect("it parses");
        assert_eq!(system.id64, 688319);
        assert_eq!(system.name, "Cygni X-3");
        assert_eq!(system.coords.x, -36417.71875);
        assert_eq!(system.coords.y, 452.0625);
        assert_eq!(system.coords.z, 6522.71875);
        assert_eq!(system.class(), Some(StarClass::H));
        assert_eq!(
            system.update_time.to_rfc3339(),
            "2026-07-27T13:33:12+00:00"
        );
    }

    /// A system nobody has looked into carries no main star, and that is a
    /// value rather than a failure.
    #[test]
    fn a_system_with_no_main_star_reads() {
        let line = r#"{"id64":1,"name":"Nowhere","coords":{"x":0,"y":0,"z":0},"updateTime":"2020-01-01T00:00:00Z"}"#;
        let system: System = serde_json::from_str(line).expect("it parses");
        assert_eq!(system.main_star, None);
        assert_eq!(system.class(), None);
    }

    /// A system whose arrival body is a planet reads as a system with no
    /// star class.
    #[test]
    fn a_rogue_planet_has_no_class() {
        let line = r#"{"id64":2,"name":"Rogue","mainStar":"Rocky body","coords":{"x":1,"y":2,"z":3},"updateTime":"2020-01-01T00:00:00Z"}"#;
        let system: System = serde_json::from_str(line).expect("it parses");
        assert_eq!(system.main_star.as_deref(), Some("Rocky body"));
        assert_eq!(system.class(), None);
    }

    /// A field the dump carries and this does not want is skipped rather
    /// than refused.
    #[test]
    fn an_unwanted_field_is_skipped() {
        let line = r#"{"id64":1,"name":"Nowhere","needsPermit":true,"coords":{"x":0,"y":0,"z":0},"updateTime":"2020-01-01T00:00:00Z"}"#;
        let system: System = serde_json::from_str(line).expect("it parses");
        assert_eq!(system.id64, 1);
    }
}
