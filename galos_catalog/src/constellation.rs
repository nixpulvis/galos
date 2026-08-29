//! The eighty-eight constellations the sky is officially divided into.
//!
//! A constellation is not a survey's measurement but the sky's agreed
//! partition of itself: the IAU carved the whole celestial sphere into 88
//! named regions in 1928, and every star sits in exactly one. So this is not
//! read from a catalog file the way [`crate::hyg`] reads stars — it is a fixed
//! table, the same for every survey, and it belongs here for the same reason
//! [`crate::frame`] does: it is part of the vocabulary a catalog is spoken in
//! rather than anything a particular file carries.
//!
//! What a catalog *does* carry is which region each of its stars falls in. HYG
//! writes it as a three-letter abbreviation in its `con` column — `Ori`,
//! `UMa`, `CMa` — and [`from_abbreviation`] turns that token into the entry in
//! this table, so a [`Star`](crate::Star) can name its constellation rather
//! than merely quote a code. That is the join that lets a consumer light up
//! Orion, or every star in the Great Bear, without teaching the renderer what
//! either of those is: the membership rides on the star, and the identity
//! lives here.
//!
//! The abbreviations are the IAU's own and match HYG's `con` column exactly —
//! all 88 of them, no more and no fewer. `Ser` (Serpens) is one entry here
//! even though the region is drawn in two disjoint halves on the sky, because
//! the IAU counts it once and so does the catalog.

/// One of the eighty-eight official constellations.
///
/// Three names for the same region: the IAU abbreviation a catalog writes, the
/// nominative a person reads, and the genitive a star's Bayer designation is
/// built from — `Orionis` in "Betelgeuse (Alpha Orionis)". All three are the
/// standard IAU forms, held together so a lookup by any one hands back the
/// others.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Constellation {
    /// The IAU three-letter abbreviation, as HYG's `con` column spells it.
    ///
    /// Mixed case and exact — `CMa`, not `cma` or `CMA` — because that is the
    /// form the catalog carries and [`from_abbreviation`] matches against it
    /// without normalising.
    pub abbreviation: &'static str,
    /// The nominative name — "Orion", "Ursa Major".
    pub name: &'static str,
    /// The genitive, which a star's designation is formed from — "Orionis",
    /// "Ursae Majoris".
    pub genitive: &'static str,
}

/// Look a constellation up by its IAU abbreviation.
///
/// The abbreviation is matched exactly, in the mixed case the IAU and HYG both
/// use, so a `con` token read straight from a catalog row resolves without any
/// massaging. A token that is not one of the 88 — a blank field, or a survey
/// that spells them differently — returns [`None`] rather than a guess.
pub fn from_abbreviation(abbreviation: &str) -> Option<&'static Constellation> {
    CONSTELLATIONS.iter().find(|c| c.abbreviation == abbreviation)
}

/// The eighty-eight constellations, in the IAU's alphabetical-by-name order.
///
/// The complete official partition of the sky. Their abbreviations are exactly
/// the set HYG uses in its `con` column, which is the guarantee that every
/// star a catalog places names a constellation that exists here.
pub const CONSTELLATIONS: [Constellation; 88] = [
    Constellation {
        abbreviation: "And",
        name: "Andromeda",
        genitive: "Andromedae",
    },
    Constellation { abbreviation: "Ant", name: "Antlia", genitive: "Antliae" },
    Constellation { abbreviation: "Aps", name: "Apus", genitive: "Apodis" },
    Constellation {
        abbreviation: "Aqr",
        name: "Aquarius",
        genitive: "Aquarii",
    },
    Constellation { abbreviation: "Aql", name: "Aquila", genitive: "Aquilae" },
    Constellation { abbreviation: "Ara", name: "Ara", genitive: "Arae" },
    Constellation { abbreviation: "Ari", name: "Aries", genitive: "Arietis" },
    Constellation { abbreviation: "Aur", name: "Auriga", genitive: "Aurigae" },
    Constellation { abbreviation: "Boo", name: "Boötes", genitive: "Boötis" },
    Constellation { abbreviation: "Cae", name: "Caelum", genitive: "Caeli" },
    Constellation {
        abbreviation: "Cam",
        name: "Camelopardalis",
        genitive: "Camelopardalis",
    },
    Constellation { abbreviation: "Cnc", name: "Cancer", genitive: "Cancri" },
    Constellation {
        abbreviation: "CVn",
        name: "Canes Venatici",
        genitive: "Canum Venaticorum",
    },
    Constellation {
        abbreviation: "CMa",
        name: "Canis Major",
        genitive: "Canis Majoris",
    },
    Constellation {
        abbreviation: "CMi",
        name: "Canis Minor",
        genitive: "Canis Minoris",
    },
    Constellation {
        abbreviation: "Cap",
        name: "Capricornus",
        genitive: "Capricorni",
    },
    Constellation { abbreviation: "Car", name: "Carina", genitive: "Carinae" },
    Constellation {
        abbreviation: "Cas",
        name: "Cassiopeia",
        genitive: "Cassiopeiae",
    },
    Constellation {
        abbreviation: "Cen",
        name: "Centaurus",
        genitive: "Centauri",
    },
    Constellation { abbreviation: "Cep", name: "Cepheus", genitive: "Cephei" },
    Constellation { abbreviation: "Cet", name: "Cetus", genitive: "Ceti" },
    Constellation {
        abbreviation: "Cha",
        name: "Chamaeleon",
        genitive: "Chamaeleontis",
    },
    Constellation {
        abbreviation: "Cir",
        name: "Circinus",
        genitive: "Circini",
    },
    Constellation {
        abbreviation: "Col",
        name: "Columba",
        genitive: "Columbae",
    },
    Constellation {
        abbreviation: "Com",
        name: "Coma Berenices",
        genitive: "Comae Berenices",
    },
    Constellation {
        abbreviation: "CrA",
        name: "Corona Australis",
        genitive: "Coronae Australis",
    },
    Constellation {
        abbreviation: "CrB",
        name: "Corona Borealis",
        genitive: "Coronae Borealis",
    },
    Constellation { abbreviation: "Crv", name: "Corvus", genitive: "Corvi" },
    Constellation { abbreviation: "Crt", name: "Crater", genitive: "Crateris" },
    Constellation { abbreviation: "Cru", name: "Crux", genitive: "Crucis" },
    Constellation { abbreviation: "Cyg", name: "Cygnus", genitive: "Cygni" },
    Constellation {
        abbreviation: "Del",
        name: "Delphinus",
        genitive: "Delphini",
    },
    Constellation { abbreviation: "Dor", name: "Dorado", genitive: "Doradus" },
    Constellation { abbreviation: "Dra", name: "Draco", genitive: "Draconis" },
    Constellation {
        abbreviation: "Equ",
        name: "Equuleus",
        genitive: "Equulei",
    },
    Constellation {
        abbreviation: "Eri",
        name: "Eridanus",
        genitive: "Eridani",
    },
    Constellation { abbreviation: "For", name: "Fornax", genitive: "Fornacis" },
    Constellation {
        abbreviation: "Gem",
        name: "Gemini",
        genitive: "Geminorum",
    },
    Constellation { abbreviation: "Gru", name: "Grus", genitive: "Gruis" },
    Constellation {
        abbreviation: "Her",
        name: "Hercules",
        genitive: "Herculis",
    },
    Constellation {
        abbreviation: "Hor",
        name: "Horologium",
        genitive: "Horologii",
    },
    Constellation { abbreviation: "Hya", name: "Hydra", genitive: "Hydrae" },
    Constellation { abbreviation: "Hyi", name: "Hydrus", genitive: "Hydri" },
    Constellation { abbreviation: "Ind", name: "Indus", genitive: "Indi" },
    Constellation {
        abbreviation: "Lac",
        name: "Lacerta",
        genitive: "Lacertae",
    },
    Constellation { abbreviation: "Leo", name: "Leo", genitive: "Leonis" },
    Constellation {
        abbreviation: "LMi",
        name: "Leo Minor",
        genitive: "Leonis Minoris",
    },
    Constellation { abbreviation: "Lep", name: "Lepus", genitive: "Leporis" },
    Constellation { abbreviation: "Lib", name: "Libra", genitive: "Librae" },
    Constellation { abbreviation: "Lup", name: "Lupus", genitive: "Lupi" },
    Constellation { abbreviation: "Lyn", name: "Lynx", genitive: "Lyncis" },
    Constellation { abbreviation: "Lyr", name: "Lyra", genitive: "Lyrae" },
    Constellation { abbreviation: "Men", name: "Mensa", genitive: "Mensae" },
    Constellation {
        abbreviation: "Mic",
        name: "Microscopium",
        genitive: "Microscopii",
    },
    Constellation {
        abbreviation: "Mon",
        name: "Monoceros",
        genitive: "Monocerotis",
    },
    Constellation { abbreviation: "Mus", name: "Musca", genitive: "Muscae" },
    Constellation { abbreviation: "Nor", name: "Norma", genitive: "Normae" },
    Constellation { abbreviation: "Oct", name: "Octans", genitive: "Octantis" },
    Constellation {
        abbreviation: "Oph",
        name: "Ophiuchus",
        genitive: "Ophiuchi",
    },
    Constellation { abbreviation: "Ori", name: "Orion", genitive: "Orionis" },
    Constellation { abbreviation: "Pav", name: "Pavo", genitive: "Pavonis" },
    Constellation { abbreviation: "Peg", name: "Pegasus", genitive: "Pegasi" },
    Constellation { abbreviation: "Per", name: "Perseus", genitive: "Persei" },
    Constellation {
        abbreviation: "Phe",
        name: "Phoenix",
        genitive: "Phoenicis",
    },
    Constellation { abbreviation: "Pic", name: "Pictor", genitive: "Pictoris" },
    Constellation { abbreviation: "Psc", name: "Pisces", genitive: "Piscium" },
    Constellation {
        abbreviation: "PsA",
        name: "Piscis Austrinus",
        genitive: "Piscis Austrini",
    },
    Constellation { abbreviation: "Pup", name: "Puppis", genitive: "Puppis" },
    Constellation { abbreviation: "Pyx", name: "Pyxis", genitive: "Pyxidis" },
    Constellation {
        abbreviation: "Ret",
        name: "Reticulum",
        genitive: "Reticuli",
    },
    Constellation {
        abbreviation: "Sge",
        name: "Sagitta",
        genitive: "Sagittae",
    },
    Constellation {
        abbreviation: "Sgr",
        name: "Sagittarius",
        genitive: "Sagittarii",
    },
    Constellation {
        abbreviation: "Sco",
        name: "Scorpius",
        genitive: "Scorpii",
    },
    Constellation {
        abbreviation: "Scl",
        name: "Sculptor",
        genitive: "Sculptoris",
    },
    Constellation { abbreviation: "Sct", name: "Scutum", genitive: "Scuti" },
    Constellation {
        abbreviation: "Ser",
        name: "Serpens",
        genitive: "Serpentis",
    },
    Constellation {
        abbreviation: "Sex",
        name: "Sextans",
        genitive: "Sextantis",
    },
    Constellation { abbreviation: "Tau", name: "Taurus", genitive: "Tauri" },
    Constellation {
        abbreviation: "Tel",
        name: "Telescopium",
        genitive: "Telescopii",
    },
    Constellation {
        abbreviation: "Tri",
        name: "Triangulum",
        genitive: "Trianguli",
    },
    Constellation {
        abbreviation: "TrA",
        name: "Triangulum Australe",
        genitive: "Trianguli Australis",
    },
    Constellation { abbreviation: "Tuc", name: "Tucana", genitive: "Tucanae" },
    Constellation {
        abbreviation: "UMa",
        name: "Ursa Major",
        genitive: "Ursae Majoris",
    },
    Constellation {
        abbreviation: "UMi",
        name: "Ursa Minor",
        genitive: "Ursae Minoris",
    },
    Constellation { abbreviation: "Vel", name: "Vela", genitive: "Velorum" },
    Constellation { abbreviation: "Vir", name: "Virgo", genitive: "Virginis" },
    Constellation { abbreviation: "Vol", name: "Volans", genitive: "Volantis" },
    Constellation {
        abbreviation: "Vul",
        name: "Vulpecula",
        genitive: "Vulpeculae",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The sky is divided into exactly eighty-eight, no more and no fewer.
    #[test]
    fn there_are_eighty_eight() {
        assert_eq!(CONSTELLATIONS.len(), 88);
    }

    /// No abbreviation is repeated, so a `con` token names one constellation
    /// and [`from_abbreviation`] cannot be ambiguous.
    #[test]
    fn every_abbreviation_is_distinct() {
        let mut abbreviations: Vec<_> =
            CONSTELLATIONS.iter().map(|c| c.abbreviation).collect();
        abbreviations.sort_unstable();
        let unique = abbreviations.len();
        abbreviations.dedup();
        assert_eq!(abbreviations.len(), unique);
    }

    /// Every entry is reachable by the token a catalog writes, and the lookup
    /// hands back the very entry it came from.
    #[test]
    fn every_constellation_is_reachable_by_its_abbreviation() {
        for constellation in &CONSTELLATIONS {
            assert_eq!(
                from_abbreviation(constellation.abbreviation),
                Some(constellation),
            );
        }
    }

    /// A well-known abbreviation resolves to its full and genitive names, the
    /// join a designation like "Alpha Orionis" is built on.
    #[test]
    fn a_known_abbreviation_resolves_to_its_names() {
        let orion = from_abbreviation("Ori").expect("Orion is a constellation");
        assert_eq!(orion.name, "Orion");
        assert_eq!(orion.genitive, "Orionis");
    }

    /// The match is exact: a token in the wrong case is not one of the 88, and
    /// a blank field is nothing rather than a guess.
    #[test]
    fn an_unknown_token_is_none() {
        assert_eq!(from_abbreviation("ORI"), None);
        assert_eq!(from_abbreviation("ori"), None);
        assert_eq!(from_abbreviation(""), None);
        assert_eq!(from_abbreviation("Xyz"), None);
    }
}
