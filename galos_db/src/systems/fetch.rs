use super::{Economies, System};
use crate::{Database, Error};
use elite_journal::prelude::*;
use galos_index::core::procedural;
use galos_index::SystemName;
use geozero::wkb;

impl System {
    pub async fn fetch(db: &Database, address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
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
                body_count,
                non_body_count,
                updated_at,
                updated_by,
                COALESCE((
                    SELECT array_agg(faction_id)
                    FROM system_factions
                    WHERE system_address = systems.address
                ), ARRAY[]::integer[]) AS "factions!"
            FROM systems
            WHERE address = $1
            "#,
            address
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(System {
            address: row.address,
            name: System::name_of(row.address, row.name)?.into_string(),
            position: row
                .position
                .map(|p| p.geometry.expect("not null or invalid")),
            population: row.population.map(|n| n as u64).unwrap_or(0),
            security: row.security,
            government: row.government,
            allegiance: row.allegiance,
            economies: Economies::new(
                row.primary_economy,
                row.secondary_economy,
            ),
            factions: row.factions,
            body_count: row.body_count,
            non_body_count: row.non_body_count,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }

    // NOTE: Assumes systems are unique by name, which is currently untrue.
    pub async fn fetch_by_name(
        db: &Database,
        name: &str,
    ) -> Result<Self, Error> {
        // Folded once, through the one type that folds a system's name, so
        // the query compares against `systems_name` rather than asking
        // Postgres to fold the column -- and so the arithmetic below is
        // asked in the case it spells, whatever case the caller typed.
        let name = SystemName::new(name);

        // A procedural name carries its own address, so resolving it here
        // turns a probe of `systems_name` into a hit on the primary key --
        // and is the only way to reach a system whose name is not written
        // down at all, which is 97.3 % of them. A name nothing spells
        // falls through to the column, where the exceptions live.
        if let Some(address) = procedural::address_of(&name) {
            return Self::fetch(db, address).await;
        }

        let row = sqlx::query!(
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
                body_count,
                non_body_count,
                updated_at,
                updated_by,
                COALESCE((
                    SELECT array_agg(faction_id)
                    FROM system_factions
                    WHERE system_address = systems.address
                ), ARRAY[]::integer[]) AS "factions!"
            FROM systems
            WHERE name = $1
            "#,
            name.as_str(),
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(System {
            address: row.address,
            name: System::name_of(row.address, row.name)?.into_string(),
            position: row
                .position
                .map(|p| p.geometry.expect("not null or invalid")),
            population: row.population.map(|n| n as u64).unwrap_or(0),
            security: row.security,
            government: row.government,
            allegiance: row.allegiance,
            economies: Economies::new(
                row.primary_economy,
                row.secondary_economy,
            ),
            factions: row.factions,
            body_count: row.body_count,
            non_body_count: row.non_body_count,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }

    pub async fn fetch_faction(
        db: &Database,
        faction: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                systems.address,
                systems.name,
                systems.position AS "position!: Option<wkb::Decode<Coordinate>>",
                systems.population,
                systems.security as "security: Security",
                systems.government as "government: Government",
                systems.allegiance as "allegiance: Allegiance",
                systems.primary_economy as "primary_economy: Economy",
                systems.secondary_economy as "secondary_economy: Economy",
                systems.body_count,
                systems.non_body_count,
                systems.updated_at,
                systems.updated_by,
                COALESCE((
                    SELECT array_agg(faction_id)
                    FROM system_factions
                    WHERE system_address = systems.address
                ), ARRAY[]::integer[]) AS "factions!"
            FROM systems
            JOIN system_factions ON system_factions.system_address = systems.address
            JOIN factions ON factions.id = system_factions.faction_id
            WHERE factions.name ILIKE $1
            "#,
            faction,
        )
        .fetch_all(&db.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(System {
                    address: row.address,
                    name: System::name_of(row.address, row.name)?.into_string(),
                    position: row
                        .position
                        .map(|p| p.geometry.expect("not null or invalid")),
                    population: row.population.map(|n| n as u64).unwrap_or(0),
                    security: row.security,
                    government: row.government,
                    allegiance: row.allegiance,
                    economies: Economies::new(
                        row.primary_economy,
                        row.secondary_economy,
                    ),
                    factions: row.factions,
                    body_count: row.body_count,
                    non_body_count: row.non_body_count,
                    updated_at: row.updated_at.and_utc(),
                    updated_by: row.updated_by,
                })
            })
            .collect()
    }
}
