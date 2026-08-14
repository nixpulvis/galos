use super::{Economies, System};
use crate::factions::{Conflict, Faction, SystemFaction};
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::{prelude::*, system::System as JournalSystem};
use geozero::wkb;

impl System {
    /// Write what a message says about a system, whenever it was sent
    ///
    /// Three things at once, which is what the `CASE` on every field below is
    /// for. The newer of two readings wins wherever they disagree, since what
    /// a system stands at changes and an older one is stale rather than a
    /// repeat. An older message still fills in what the row has never held,
    /// since a blank is not a reading it can contradict: a scan writes a
    /// system with nothing but a name and a place, and the visit that says who
    /// holds it may well be the one delivered late. And the stamp holds at the
    /// newest reading either way, so a late message does not put the row back
    /// to when it was sent.
    ///
    /// [`Self::from_journal`] asks the same, and must: the two write one row.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        db: &Database,
        address: i64,
        name: &str,
        position: Option<Coordinate>,
        primary_star_class: Option<String>,
        population: Option<u64>,
        security: Option<Security>,
        government: Option<Government>,
        allegiance: Option<Allegiance>,
        economies: Option<Economies>,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<(), Error> {
        sqlx::query!(
            r#"
            INSERT INTO systems
                (address,
                 name,
                 primary_star_class,
                 position,
                 population,
                 security,
                 government,
                 allegiance,
                 primary_economy,
                 secondary_economy,
                 updated_at,
                 updated_by)
            VALUES ($1, UPPER($2), $3, $4::geometry, $5, $6,
                $7, $8, $9, $10, $11, $12)
            ON CONFLICT (address)
            DO UPDATE SET
                primary_star_class = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($3, systems.primary_star_class)
                    ELSE COALESCE(systems.primary_star_class, $3) END,
                position = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($4, systems.position)
                    ELSE COALESCE(systems.position, $4) END,
                population = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($5, systems.population)
                    ELSE COALESCE(systems.population, $5) END,
                security = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($6, systems.security)
                    ELSE COALESCE(systems.security, $6) END,
                government = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($7, systems.government)
                    ELSE COALESCE(systems.government, $7) END,
                allegiance = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($8, systems.allegiance)
                    ELSE COALESCE(systems.allegiance, $8) END,
                primary_economy = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($9, systems.primary_economy)
                    ELSE COALESCE(systems.primary_economy, $9) END,
                secondary_economy = CASE WHEN $11 >= systems.updated_at
                    THEN COALESCE($10, systems.secondary_economy)
                    ELSE COALESCE(systems.secondary_economy, $10) END,
                updated_at = GREATEST(systems.updated_at, $11),
                updated_by = CASE WHEN $11 >= systems.updated_at
                    THEN $12 ELSE systems.updated_by END
            "#,
            address as i64,
            name,
            primary_star_class,
            position.map(|p| wkb::Encode(p)) as _,
            population.map(|n| n as i64),
            security as _,
            government as _,
            allegiance as _,
            economies.map(|economies| economies.primary) as _,
            economies.and_then(|economies| economies.secondary) as _,
            updated_at.naive_utc(),
            updated_by
        )
        .execute(&db.pool)
        .await?;

        Self::adopt_waiting_markets(db, address, name, updated_at, updated_by)
            .await?;

        Ok(())
    }

    /// Link up any markets that named this system before it existed
    ///
    /// A market message gives a system name and no address, so its market is
    /// recorded unlinked and waits. This is the moment that wait can end, so
    /// it is answered here rather than swept for later.
    ///
    /// The station has to exist before a market may point at it, because the
    /// foreign key onto it stops being satisfied by a null the instant the
    /// address is filled in.
    async fn adopt_waiting_markets(
        db: &Database,
        address: i64,
        name: &str,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<(), Error> {
        // This runs on every system write, including the inner loop of the
        // bulk importers, and almost always there is nothing waiting. Ask
        // the partial index before opening a transaction for no reason.
        let waiting = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM markets
                 WHERE system_address IS NULL AND system_name = UPPER($1)
            ) AS "waiting!"
            "#,
            name,
        )
        .fetch_one(&db.pool)
        .await?;

        if !waiting {
            return Ok(());
        }

        let mut tx = db.pool.begin().await?;

        sqlx::query!(
            r#"
            INSERT INTO stations (system_address, name, updated_at, updated_by)
            SELECT $1, m.station_name, $3, $4
              FROM markets m
             WHERE m.system_address IS NULL AND m.system_name = UPPER($2)
            ON CONFLICT (system_address, name) DO NOTHING
            "#,
            address,
            name,
            updated_at.naive_utc(),
            updated_by,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            r#"
            UPDATE markets SET system_address = $1
             WHERE system_address IS NULL AND system_name = UPPER($2)
            "#,
            address,
            name,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        system: &JournalSystem,
    ) -> Result<(), Error> {
        let position =
            system.pos.map(|p| Coordinate { x: p.x, y: p.y, z: p.z });
        let economies = Economies::new(system.economy, system.second_economy);
        sqlx::query!(
            r#"
            INSERT INTO systems
                (address,
                 name,
                 position,
                 population,
                 security,
                 government,
                 allegiance,
                 primary_economy,
                 secondary_economy,
                 updated_at,
                 updated_by)
            VALUES ($1, UPPER($2), $3::geometry, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (address)
            DO UPDATE SET
                position = CASE WHEN $10 >= systems.updated_at
                    THEN COALESCE($3, systems.position)
                    ELSE COALESCE(systems.position, $3) END,
                population = CASE WHEN $10 >= systems.updated_at
                    THEN COALESCE($4, systems.population)
                    ELSE COALESCE(systems.population, $4) END,
                security = CASE WHEN $10 >= systems.updated_at
                    THEN COALESCE($5, systems.security)
                    ELSE COALESCE(systems.security, $5) END,
                government = CASE WHEN $10 >= systems.updated_at
                    THEN COALESCE($6, systems.government)
                    ELSE COALESCE(systems.government, $6) END,
                allegiance = CASE WHEN $10 >= systems.updated_at
                    THEN COALESCE($7, systems.allegiance)
                    ELSE COALESCE(systems.allegiance, $7) END,
                primary_economy = CASE WHEN $10 >= systems.updated_at
                    THEN COALESCE($8, systems.primary_economy)
                    ELSE COALESCE(systems.primary_economy, $8) END,
                secondary_economy = CASE WHEN $10 >= systems.updated_at
                    THEN COALESCE($9, systems.secondary_economy)
                    ELSE COALESCE(systems.secondary_economy, $9) END,
                updated_at = GREATEST(systems.updated_at, $10),
                updated_by = CASE WHEN $10 >= systems.updated_at
                    THEN $11 ELSE systems.updated_by END
            "#,
            system.address as i64,
            system.name,
            position.map(|p| wkb::Encode(p)) as _,
            system.population.map(|n| n as i64),
            system.security as _,
            system.government as _,
            system.allegiance as _,
            economies.map(|economies| economies.primary) as _,
            economies.and_then(|economies| economies.secondary) as _,
            timestamp.naive_utc(),
            user
        )
        .execute(&db.pool)
        .await?;

        // Asked whatever became of the row above. A faction and a conflict are
        // rows of their own, each with a stamp of its own and its own say in
        // whether a message is worth taking, and a market waiting on this
        // system waits on the system existing rather than on this message
        // being the newest thing said about it.
        for faction in &system.factions {
            let faction_id = Faction::create(db, &faction.name).await?.id;
            SystemFaction::from_journal(
                db,
                system.address,
                faction_id as u32,
                &faction,
                timestamp,
            )
            .await?;
        }

        for conflict in &system.conflicts {
            Conflict::from_journal(db, system.address, &conflict, timestamp)
                .await?;
        }

        Self::adopt_waiting_markets(
            db,
            system.address,
            &system.name,
            timestamp,
            user,
        )
        .await?;

        Ok(())
    }

    /// Record how many bodies a system holds
    ///
    /// Three events report this and they are reporting the same number, so
    /// they arrive here together. `non_body_count` is the belts and rings,
    /// which only the honk counts; the others pass [`None`] and leave
    /// whatever is there alone.
    ///
    /// The system need not be on record. A honk is often the first thing
    /// heard about somewhere, and carries a name and a position, which is
    /// enough to write the row it belongs to.
    ///
    /// Unlike [`System::create`] this does not refuse an older message. A
    /// count does not go stale -- a system does not gain or lose bodies --
    /// and the timestamp guard would throw nearly all of them away, since a
    /// system busy enough to be honked at is busy enough to have been written
    /// more recently by something else. What an older message does not do is
    /// put the system's reading back to when it was sent.
    pub async fn set_body_counts(
        db: &Database,
        address: i64,
        name: &str,
        position: Option<Coordinate>,
        body_count: i32,
        non_body_count: Option<i32>,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<(), Error> {
        sqlx::query!(
            r#"
            INSERT INTO systems
                (address,
                 name,
                 position,
                 body_count,
                 non_body_count,
                 updated_at,
                 updated_by)
            VALUES ($1, UPPER($2), $3::geometry, $4, $5, $6, $7)
            ON CONFLICT (address)
            DO UPDATE SET
                position = COALESCE($3, systems.position),
                body_count = $4,
                non_body_count =
                    COALESCE($5, systems.non_body_count),
                updated_at = GREATEST(systems.updated_at, $6),
                updated_by = CASE WHEN $6 >= systems.updated_at
                    THEN $7 ELSE systems.updated_by END
            "#,
            address,
            name,
            position.map(|p| wkb::Encode(p)) as _,
            body_count,
            non_body_count,
            updated_at.naive_utc(),
            updated_by,
        )
        .execute(&db.pool)
        .await?;

        Self::adopt_waiting_markets(db, address, name, updated_at, updated_by)
            .await?;

        Ok(())
    }
}
