//! NPC market dynamics: consumption drains stock (prices recover along the
//! curve), and debt accrues interest. NPC factories and contract issuance
//! arrive in a later milestone.

use super::*;
use bevy::prelude::*;

pub fn market_tick(
    mut credits: ResMut<Credits>,
    debt: Res<Debt>,
    mut markets: Query<&mut Market>,
) {
    for mut market in markets.iter_mut() {
        for entry in market.entries.values_mut() {
            entry.consum_accum_milli += entry.consumption_milli;
            let units = entry.consum_accum_milli / 1000;
            if units > 0 {
                entry.consum_accum_milli -= units * 1000;
                entry.stock = entry.stock.saturating_sub(units);
            }
        }
    }

    if credits.0 < 0 && debt.interest_milli > 0 {
        let interest = (-credits.0) * debt.interest_milli as i64 / 1_000_000;
        credits.0 -= interest.max(0);
    }
}
