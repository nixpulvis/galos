//! What is at the middle of a system, as the dump says it and as the game
//! says it.
//!
//! Spansh writes `mainStar` as prose — `"M (Red super giant) Star"`,
//! `"White Dwarf (DAZ) Star"` — where the game writes a token:
//! `M_RedSuperGiant`, `DAZ`. This is the translation, into
//! [`elite_journal::body::StarClass`].
//!
//! It is exhaustive and lossless against the closed `enum` the published
//! schema declares, all sixty-one values of it;
//! `every_value_the_schema_lists_is_mapped` holds it to that list. Size is
//! kept: `"B (Blue-White super giant) Star"` is
//! `StarClass::B(SuperGiant)`, and a supergiant and a dwarf of the same
//! letter differ by five magnitudes.
//!
//! Eighteen of the sixty-one are not stars — `"Rocky body"`, `"Class I gas
//! giant"`, `"Water world"` and the rest — which is what a rogue planet or
//! an unscanned system looks like from outside. Those answer [`None`],
//! the same answer a system nobody has looked into gives.

use elite_journal::body::{StarClass, StarSize};

/// The game's class for a dump's `mainStar`, or [`None`] where what is at
/// the middle of the system is not a star.
///
/// Unrecognised prose also answers [`None`]: the schema is a closed
/// `enum`, so a value not in it means the dump's format moved. This is not
/// [`StarClass::Unknown`], which is for a token the game wrote and this
/// crate has no name for.
pub fn class_of(main_star: &str) -> Option<StarClass> {
    Some(match main_star {
        // The main sequence, and the giants and supergiants that share its
        // letters. The game keeps the size and so does this.
        "O (Blue-White) Star" => StarClass::O(StarSize::Dwarf),
        "B (Blue-White) Star" => StarClass::B(StarSize::Dwarf),
        "B (Blue-White super giant) Star" => StarClass::B(StarSize::SuperGiant),
        "A (Blue-White) Star" => StarClass::A(StarSize::Dwarf),
        "A (Blue-White super giant) Star" => StarClass::A(StarSize::SuperGiant),
        "F (White) Star" => StarClass::F(StarSize::Dwarf),
        "F (White super giant) Star" => StarClass::F(StarSize::SuperGiant),
        "G (White-Yellow) Star" => StarClass::G(StarSize::Dwarf),
        "G (White-Yellow super giant) Star" => {
            StarClass::G(StarSize::SuperGiant)
        }
        "K (Yellow-Orange) Star" => StarClass::K(StarSize::Dwarf),
        "K (Yellow-Orange giant) Star" => StarClass::K(StarSize::Giant),
        "M (Red dwarf) Star" => StarClass::M(StarSize::Dwarf),
        "M (Red giant) Star" => StarClass::M(StarSize::Giant),
        "M (Red super giant) Star" => StarClass::M(StarSize::SuperGiant),

        // The brown dwarfs, which the game gives no size to spell.
        "L (Brown dwarf) Star" => StarClass::L(StarSize::Dwarf),
        "T (Brown dwarf) Star" => StarClass::T(StarSize::Dwarf),
        "Y (Brown dwarf) Star" => StarClass::Y(StarSize::Dwarf),

        // Pre-main-sequence.
        "T Tauri Star" => StarClass::TTauri,
        "Herbig Ae/Be Star" => StarClass::HerbigAeBe,

        // The S and MS giants, read whole because their tokens collide with
        // the main sequence.
        "S-type Star" => StarClass::S,
        "MS-type Star" => StarClass::MS,

        // Carbon stars.
        "C Star" => StarClass::C,
        "CJ Star" => StarClass::CJ,
        "CN Star" => StarClass::CN,

        // Wolf-Rayet.
        "Wolf-Rayet Star" => StarClass::W,
        "Wolf-Rayet C Star" => StarClass::WC,
        "Wolf-Rayet N Star" => StarClass::WN,
        "Wolf-Rayet NC Star" => StarClass::WNC,
        "Wolf-Rayet O Star" => StarClass::WO,

        // White dwarfs, by their spectrum.
        "White Dwarf (D) Star" => StarClass::D,
        "White Dwarf (DA) Star" => StarClass::DA,
        "White Dwarf (DAB) Star" => StarClass::DAB,
        "White Dwarf (DAV) Star" => StarClass::DAV,
        "White Dwarf (DAZ) Star" => StarClass::DAZ,
        "White Dwarf (DB) Star" => StarClass::DB,
        "White Dwarf (DBV) Star" => StarClass::DBV,
        "White Dwarf (DBZ) Star" => StarClass::DBZ,
        "White Dwarf (DC) Star" => StarClass::DC,
        "White Dwarf (DCV) Star" => StarClass::DCV,
        "White Dwarf (DQ) Star" => StarClass::DQ,

        // The remnants.
        "Neutron Star" => StarClass::N,
        "Black Hole" => StarClass::H,
        "Supermassive Black Hole" => StarClass::SupermassiveBlackHole,

        // Not a star at all: the arrival body is a planet. See the module
        // header for why this is nothing rather than something dark.
        "Ammonia world"
        | "Class I gas giant"
        | "Class II gas giant"
        | "Class III gas giant"
        | "Class IV gas giant"
        | "Class V gas giant"
        | "Earth-like world"
        | "Gas giant with ammonia-based life"
        | "Gas giant with water-based life"
        | "Helium gas giant"
        | "Helium-rich gas giant"
        | "High metal content world"
        | "Icy body"
        | "Metal-rich body"
        | "Rocky Ice world"
        | "Rocky body"
        | "Water giant"
        | "Water world" => return None,

        // The schema is closed, so this is the format having moved.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value `systems.schema.json` lists, all sixty-one.
    ///
    /// Copied from the published schema, which is what the format
    /// promises, rather than from the file.
    const LISTED: [&str; 61] = [
        "A (Blue-White super giant) Star",
        "A (Blue-White) Star",
        "Ammonia world",
        "B (Blue-White super giant) Star",
        "B (Blue-White) Star",
        "Black Hole",
        "C Star",
        "CJ Star",
        "CN Star",
        "Class I gas giant",
        "Class II gas giant",
        "Class III gas giant",
        "Class IV gas giant",
        "Class V gas giant",
        "Earth-like world",
        "F (White super giant) Star",
        "F (White) Star",
        "G (White-Yellow super giant) Star",
        "G (White-Yellow) Star",
        "Gas giant with ammonia-based life",
        "Gas giant with water-based life",
        "Helium gas giant",
        "Helium-rich gas giant",
        "Herbig Ae/Be Star",
        "High metal content world",
        "Icy body",
        "K (Yellow-Orange giant) Star",
        "K (Yellow-Orange) Star",
        "L (Brown dwarf) Star",
        "M (Red dwarf) Star",
        "M (Red giant) Star",
        "M (Red super giant) Star",
        "MS-type Star",
        "Metal-rich body",
        "Neutron Star",
        "O (Blue-White) Star",
        "Rocky Ice world",
        "Rocky body",
        "S-type Star",
        "Supermassive Black Hole",
        "T (Brown dwarf) Star",
        "T Tauri Star",
        "Water giant",
        "Water world",
        "White Dwarf (D) Star",
        "White Dwarf (DA) Star",
        "White Dwarf (DAB) Star",
        "White Dwarf (DAV) Star",
        "White Dwarf (DAZ) Star",
        "White Dwarf (DB) Star",
        "White Dwarf (DBV) Star",
        "White Dwarf (DBZ) Star",
        "White Dwarf (DC) Star",
        "White Dwarf (DCV) Star",
        "White Dwarf (DQ) Star",
        "Wolf-Rayet C Star",
        "Wolf-Rayet N Star",
        "Wolf-Rayet NC Star",
        "Wolf-Rayet O Star",
        "Wolf-Rayet Star",
        "Y (Brown dwarf) Star",
    ];

    /// The eighteen listed values whose main body is not a star.
    const NOT_A_STAR: [&str; 18] = [
        "Ammonia world",
        "Class I gas giant",
        "Class II gas giant",
        "Class III gas giant",
        "Class IV gas giant",
        "Class V gas giant",
        "Earth-like world",
        "Gas giant with ammonia-based life",
        "Gas giant with water-based life",
        "Helium gas giant",
        "Helium-rich gas giant",
        "High metal content world",
        "Icy body",
        "Metal-rich body",
        "Rocky Ice world",
        "Rocky body",
        "Water giant",
        "Water world",
    ];

    /// Every value the schema lists is accounted for: a star gets a class
    /// and a planet gets nothing, and neither falls through to the arm that
    /// means "the format moved".
    ///
    /// A value missed here would light a whole family of system as the
    /// fallback red dwarf, silently.
    #[test]
    fn every_value_the_schema_lists_is_mapped() {
        for listed in LISTED {
            let mapped = class_of(listed);
            assert_eq!(
                mapped.is_some(),
                !NOT_A_STAR.contains(&listed),
                "{listed} mapped to {mapped:?}",
            );
        }
        // And the second list really is part of the first, so a typo in it
        // cannot quietly excuse a star from the check above.
        for body in NOT_A_STAR {
            assert!(LISTED.contains(&body), "{body} is not a listed value");
        }
    }

    /// Nothing is translated into a class the game does not have a token
    /// for: an `Unknown` here would mean this crate invented a spelling.
    #[test]
    fn nothing_maps_to_an_unknown_class() {
        for listed in LISTED {
            if let Some(class) = class_of(listed) {
                assert!(
                    !matches!(class, StarClass::Unknown(_)),
                    "{listed} mapped to an unknown class",
                );
            }
        }
    }

    /// The prose's distinctions survive: a supergiant does not become a
    /// dwarf of the same letter.
    #[test]
    fn a_supergiant_keeps_its_size() {
        assert_eq!(
            class_of("M (Red dwarf) Star"),
            Some(StarClass::M(StarSize::Dwarf)),
        );
        assert_eq!(
            class_of("M (Red giant) Star"),
            Some(StarClass::M(StarSize::Giant)),
        );
        assert_eq!(
            class_of("M (Red super giant) Star"),
            Some(StarClass::M(StarSize::SuperGiant)),
        );
        assert_eq!(
            class_of("B (Blue-White super giant) Star"),
            Some(StarClass::B(StarSize::SuperGiant)),
        );
        assert_eq!(
            class_of("K (Yellow-Orange giant) Star"),
            Some(StarClass::K(StarSize::Giant)),
        );
        // And all three M's are an M.
        for prose in [
            "M (Red dwarf) Star",
            "M (Red giant) Star",
            "M (Red super giant) Star",
        ] {
            assert!(
                matches!(class_of(prose), Some(StarClass::M(_))),
                "{prose}"
            );
        }
    }

    /// The tokens are the game's, which is what makes a dump's system and a
    /// journal's the same value.
    #[test]
    fn the_tokens_are_the_games() {
        let token = |prose| class_of(prose).map(|it| it.token().to_owned());
        assert_eq!(token("White Dwarf (DAZ) Star").as_deref(), Some("DAZ"));
        assert_eq!(token("Wolf-Rayet NC Star").as_deref(), Some("WNC"));
        assert_eq!(token("T Tauri Star").as_deref(), Some("TTS"));
        assert_eq!(token("Herbig Ae/Be Star").as_deref(), Some("AeBe"));
        assert_eq!(token("Neutron Star").as_deref(), Some("N"));
        assert_eq!(token("Black Hole").as_deref(), Some("H"));
        assert_eq!(
            token("Supermassive Black Hole").as_deref(),
            Some("SupermassiveBlackHole"),
        );
        assert_eq!(token("MS-type Star").as_deref(), Some("MS"));
        assert_eq!(
            token("B (Blue-White super giant) Star").as_deref(),
            Some("B_BlueWhiteSuperGiant"),
        );
    }

    /// A value the schema does not list is the format having moved, and is
    /// not guessed at.
    #[test]
    fn an_unknown_value_is_not_guessed() {
        assert_eq!(class_of("Q (Quite New) Star"), None);
        assert_eq!(class_of(""), None);
    }
}
