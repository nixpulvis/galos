//! Who runs each system, re-derived every tick.
//!
//! The controlling faction is whoever holds the most influence *right now*
//! — exactly as in the real BGS — so a live sync that shifts influence
//! shifts taxes and productivity without anything else being recomputed.

use super::*;
use bevy::prelude::*;
use elite_journal::faction::{Happiness, State};

pub fn resolve_control(
    mut systems: Query<(Entity, &mut Control)>,
    presences: Query<&Presence>,
) {
    for (system, mut control) in systems.iter_mut() {
        let controlling = presences
            .iter()
            .filter(|presence| presence.system == system)
            .max_by(|a, b| a.influence.total_cmp(&b.influence));

        let Some(presence) = controlling else {
            *control = Control::default();
            continue;
        };

        // A dominant faction runs a tight, cheap system; a fractured one
        // taxes harder to hold on.
        let tax_milli = match presence.influence {
            i if i >= 60.0 => 25,
            i if i >= 40.0 => 50,
            i if i >= 20.0 => 75,
            _ => 100,
        };
        // Happier workforces build faster; an unknown band is neutral.
        let productivity_milli = match presence.happiness {
            Happiness::Elated => 1100,
            Happiness::Happy => 1050,
            Happiness::Discontented | Happiness::None => 1000,
            Happiness::Unhappy => 900,
            Happiness::Despondent => 800,
        };

        *control = Control {
            faction: Some(presence.faction),
            tax_milli,
            productivity_milli,
            boom: matches!(presence.state, State::Boom),
        };
    }
}
