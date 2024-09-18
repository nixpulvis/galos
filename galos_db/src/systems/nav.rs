use super::System;
use crate::Database;
use async_std::task;
use elite_journal::prelude::*;
use geozero::wkb;
use ordered_float::OrderedFloat;
use pathfinding::prelude::*;
use std::str::FromStr;

impl System {
    pub fn neighbors(&self, db: &Database, range: f64) -> Vec<System> {
        let rows = task::block_on(async {
            sqlx::query!(
                r#"
                SELECT
                    address,
                    name,
                    position AS "position!: Option<wkb::Decode<Coordinate>>",
                    population,
                    security as "security: Security",
                    government as "government: Government",
                    allegiance as "allegiance: Allegiance",
                    primary_economy as "primary_economy: Economy",
                    secondary_economy as "secondary_economy: Economy",
                    updated_at,
                    updated_by
                FROM systems
                WHERE ST_3DDWithin(position, $1, $2);
                "#,
                self.position.map(|p| wkb::Encode(p)) as _,
                range
            )
            .fetch_all(&db.pool)
            .await
            .unwrap()
        });

        rows.into_iter()
            .map(|row| System {
                address: row.address,
                name: row.name,
                position: row
                    .position
                    .map(|p| p.geometry.expect("not null or invalid")),
                population: row.population.map(|n| n as u64).unwrap_or(0),
                security: row.security,
                government: row.government,
                allegiance: row.allegiance,
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect()
    }

    pub fn distance(&self, other: &System) -> f64 {
        if let (Some(p1), Some(p2)) = (self.position, other.position) {
            ((p2.x - p1.x).powi(2) + (p2.y - p1.y).powi(2) + (p2.z - p1.z).powi(2)).sqrt()
        } else {
            0.
        }
    }

    pub fn route_to(
        &self,
        db: &Database,
        end: &System,
        range: f64,
    ) -> Option<(Vec<Self>, OrderedFloat<f64>)> {
        let successors = |s: &System| {
            s.neighbors(db, range)
                .into_iter()
                .map(|s| (s, OrderedFloat(1.)))
        };

        // Making the heuristic much larger than the successor's jump cost makes things run
        // faster, but is not optimal...
        let heuristic = |s: &System| OrderedFloat((s.distance(end) / range).ceil());

        let success = |s: &System| s == end;

        astar(self, successors, heuristic, success)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ModuleClass {
    A,
    B,
    C,
    D,
    E,
}

impl FromStr for ModuleClass {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            "D" => Ok(Self::D),
            "E" => Ok(Self::E),
            _ => Err("invalid class".to_string()),
        }
    }
}

// https://www.reddit.com/r/EliteDangerous/comments/30nx4u/the_hyperspace_fuel_equation_documented
pub fn _fuel_cost(
    distance: f64,
    mass: f64,
    optimal_mass: f64,
    size: u8,
    class: ModuleClass,
) -> f64 {
    let l = match class {
        ModuleClass::A => 12.,
        ModuleClass::B => 10.,
        ModuleClass::C => 8.,
        ModuleClass::D => 10.,
        ModuleClass::E => 11.,
    };

    let p = match size {
        2 => 2.,
        3 => 2.15,
        4 => 2.3,
        5 => 2.45,
        6 => 2.6,
        7 => 2.75,
        8 => 2.9,
        _ => panic!("bad size"),
    };

    l * 0.001 * (distance * mass / optimal_mass).powf(p)
}
