//! The index's own parts, named once.
//!
//! What a directory holds of its own, each part a file or a set of them that
//! a build writes and a repair can write alone. The tables a dependent
//! contributes are parts too, named by [`crate::records::Table::NAME`]; these
//! are the ones every directory has whatever it was built for.

/// One of the index's own parts.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CorePart {
    /// The cell tree and its payloads.
    Cells,
    /// Every system's name and place.
    Names,
    /// The populated systems.
    Populated,
    /// How far each scanned system reaches.
    Reaches,
    /// The faction id-to-name table.
    Factions,
    /// One file per system of what was scanned in it.
    Bodies,
}

impl CorePart {
    /// Every one of them, in the order a reader is told them.
    pub const ALL: [CorePart; 6] = [
        CorePart::Cells,
        CorePart::Names,
        CorePart::Populated,
        CorePart::Reaches,
        CorePart::Factions,
        CorePart::Bodies,
    ];

    /// What it is called, where an operator names it.
    pub fn name(self) -> &'static str {
        match self {
            CorePart::Cells => "cells",
            CorePart::Names => "names",
            CorePart::Populated => "populated",
            CorePart::Reaches => "reaches",
            CorePart::Factions => "factions",
            CorePart::Bodies => "bodies",
        }
    }

    /// What it is for, in a line.
    pub fn about(self) -> &'static str {
        match self {
            CorePart::Cells => {
                "The cell tree and its payloads, which the map draws the \
                 galaxy from."
            }
            CorePart::Names => {
                "Every system's name and place: the search index and the \
                 routing graph."
            }
            CorePart::Populated => {
                "The populated systems the map colors and filters by."
            }
            CorePart::Reaches => {
                "How far each scanned system reaches, which every shell is \
                 sized by."
            }
            CorePart::Factions => "The faction id-to-name table.",
            CorePart::Bodies => {
                "One file per system of the stars, bodies and barycenters in \
                 it."
            }
        }
    }

    /// The part called `name`, if one is.
    pub fn named(name: &str) -> Option<CorePart> {
        CorePart::ALL.into_iter().find(|it| it.name() == name)
    }
}
