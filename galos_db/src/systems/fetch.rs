use super::{Economies, System};
use crate::{escaped, Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::prelude::*;
use geozero::wkb;
use std::collections::HashMap;

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
            name: row.name,
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
            name.to_uppercase()
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(System {
            address: row.address,
            name: row.name,
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

    pub async fn fetch_like_name(
        db: &Database,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
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
            WHERE name ILIKE $1
            ORDER BY name
            "#,
            name
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
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
            .collect())
    }

    /// The systems whose names hold `query`, best first
    ///
    /// What a search box asks, where the user is part way through typing a
    /// name and wants to be shown which systems they might mean.
    ///
    /// `query` is read as letters rather than as a pattern. A name is a thing
    /// the user is halfway through typing, so `%` and `_` in it are
    /// characters they typed and not wildcards they meant, and [`escaped`]
    /// takes them at their word. That is the difference between this and
    /// [`System::fetch_like_name`], which takes a pattern whole from whoever
    /// wrote it.
    ///
    /// Ordered so that the `limit` keeps the rows worth keeping. The name
    /// spelled out in full first, so that a user who typed the whole of one is
    /// answered with it wherever it happens to lie; ordering rather than a
    /// lookup of its own, since the `LIMIT` cuts what the order has already
    /// settled and the top of that order is a place nothing can crowd it out
    /// of. Then names that start with the query, since someone typing `sol`
    /// means Sol before they mean Nasolituw. Then nearest to `near`, which is
    /// where the user is looking: a common fragment matches tens of thousands
    /// of systems and the ones they mean are the ones in front of them.
    /// Systems with no position on record sort last of all, having no distance
    /// to be near by and nowhere to be flown to.
    ///
    /// Bounded because it has to be. A query of a letter or two matches most
    /// of the systems on record, `%a%` reaching two in three of the hundreds
    /// of thousands held, and a list nobody can read to the end of is no more
    /// use for being complete.
    ///
    /// A share rather than a count of them. The ingest adds systems for as
    /// long as it runs, so a number written here is right on the day and drifts
    /// from then on, where what the bound is argued from is the share and that
    /// holds.
    pub async fn search_by_name(
        db: &Database,
        query: &str,
        near: Option<Coordinate>,
        limit: i64,
    ) -> Result<Vec<Self>, Error> {
        let query = escaped(query);
        let rows = sqlx::query!(
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
            -- $1 decides which rows match and $2 and $3 decide which of those
            -- come first: held anywhere in the name to match, the whole of the
            -- name to lead, held at the start of it to come next. Three
            -- patterns because they answer three questions, and the last two
            -- are asked in SQL rather than after the rows arrive because
            -- ordering is what the LIMIT cuts against.
            WHERE name ILIKE $1
            ORDER BY
                (name ILIKE $2) DESC,
                (name ILIKE $3) DESC,
                position <<->> $4::geometry NULLS LAST,
                name
            LIMIT $5
            "#,
            format!("%{query}%"),
            query,
            format!("{query}%"),
            near.map(wkb::Encode) as _,
            limit
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
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
            .collect())
    }

    pub async fn fetch_in_range_by_name(
        db: &Database,
        range: f64,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                s1.address,
                s1.name,
                s1.position AS "position!: Option<wkb::Decode<Coordinate>>",
                s1.population,
                s1.security as "security: Security",
                s1.government as "government: Government",
                s1.allegiance as "allegiance: Allegiance",
                s1.primary_economy as "primary_economy: Economy",
                s1.secondary_economy as "secondary_economy: Economy",
                s1.body_count,
                s1.non_body_count,
                s1.updated_at,
                s1.updated_by,
                COALESCE((
                    SELECT array_agg(faction_id)
                    FROM system_factions
                    WHERE system_address = s1.address
                ), ARRAY[]::integer[]) AS "factions!"
            FROM systems s1
            FULL JOIN systems s2 ON ST_3DDWithin(s1.position, s2.position, $2)
            WHERE s2.name = $1
            ORDER BY ST_3DDistance(s1.position, s2.position)
            "#,
            name.to_uppercase(),
            range
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
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
            .collect())
    }

    pub async fn fetch_in_range_like_name(
        db: &Database,
        range: f64,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT
                s1.address,
                s1.name,
                s1.position AS "position!: Option<wkb::Decode<Coordinate>>",
                s1.population,
                s1.security as "security: Security",
                s1.government as "government: Government",
                s1.allegiance as "allegiance: Allegiance",
                s1.primary_economy as "primary_economy: Economy",
                s1.secondary_economy as "secondary_economy: Economy",
                s1.body_count,
                s1.non_body_count,
                s1.updated_at,
                s1.updated_by,
                COALESCE((
                    SELECT array_agg(faction_id)
                    FROM system_factions
                    WHERE system_address = s1.address
                ), ARRAY[]::integer[]) AS "factions!"
            FROM systems s1
            FULL JOIN systems s2 ON ST_3DDWithin(s1.position, s2.position, $2)
            WHERE s2.name ILIKE $1
            ORDER BY ST_3DDistance(s1.position, s2.position)
            "#,
            name,
            range
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
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
            .collect())
    }

    /// Every system within `range` of `center`, in light years
    ///
    /// The map's widest question, and the one asked over and over: the
    /// spyglass puts it every time the camera settles somewhere new. What it
    /// costs is set by how many systems come back, which at the far end of the
    /// radius is most of the table.
    ///
    /// `admitting` narrows it to what the map is going to draw, as a list of
    /// factions and a list of addresses. A system is admitted by standing in
    /// either, since each thing the user has asked to see adds to what is
    /// shown rather than cutting into it. Nothing narrows it to nothing: the
    /// whole region, which is what the map asks for when it is drawing the
    /// whole sky.
    ///
    /// Narrowed, it is asked in two halves: which systems are admitted, and
    /// then what is known about those. The second half is [`Self::fetch_many`]
    /// unchanged, which is what turns addresses into systems everywhere else,
    /// so what a system is made of is written out once however it was reached.
    /// The extra round trip is affordable for exactly the reason the narrowing
    /// is worth doing: what comes back is the handful being drawn rather than
    /// the hundred thousand around them.
    ///
    /// Who is present in each system is asked separately, by
    /// `system_factions`, and joined up here. Asked of every row in
    /// the same query it is a subquery run once per system: over a hundred
    /// thousand systems that is six parts in seven of the whole query, spent
    /// discovering that all but three systems in a hundred have nobody on
    /// record at all. Narrowed there are few enough rows for `fetch_many` to
    /// go on asking it the cheap way.
    ///
    /// Asked of the addresses that came back rather than of the region again.
    /// `ST_3DDWithin` estimates its own rows so far under that a second query
    /// naming the region is planned as a nested loop and takes an order of
    /// magnitude longer than asking for the whole table. The addresses are in
    /// hand by then and say the same thing without the planner having to
    /// guess at it.
    pub async fn fetch_in_range_of_point(
        db: &Database,
        range: f64,
        center: [f64; 3],
        admitting: Option<(&[i32], &[i64])>,
    ) -> Result<Vec<Self>, Error> {
        if let Some((factions, addresses)) = admitting {
            let admitted =
                Self::admitted_in_range(db, range, center, factions, addresses)
                    .await?;
            return Self::fetch_many(db, &admitted).await;
        }

        let rows = sqlx::query!(
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
                updated_by
            FROM systems
            WHERE ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $1)
            "#,
            range,
            center[0],
            center[1],
            center[2],
        )
        .fetch_all(&db.pool)
        .await?;

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut present = Self::system_factions(db, &found).await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                factions: present.remove(&row.address).unwrap_or_default(),
                body_count: row.body_count,
                non_body_count: row.non_body_count,
                address: row.address,
                name: row.name,
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
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }

    /// Which systems within `range` of `center` are admitted, by address
    ///
    /// Admitted by standing at one of `addresses`, or by having one of
    /// `factions` present in it.
    ///
    /// The two are named apart and put together, rather than being one
    /// `WHERE` with an `OR` down the middle. An `OR` leaves the planner
    /// nothing to drive from but the region, so it walks every system in range
    /// asking who is in it, and takes longer than fetching the lot. Apart, the
    /// faction half drives from `system_factions` and reaches its systems by
    /// their addresses.
    ///
    /// Put together on addresses, which is the one column that says which
    /// system a row is about, so a system admitted twice is named once.
    async fn admitted_in_range(
        db: &Database,
        range: f64,
        center: [f64; 3],
        factions: &[i32],
        addresses: &[i64],
    ) -> Result<Vec<i64>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT address
            FROM systems
            WHERE ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $1)
              AND address = ANY($5)
            UNION
            SELECT systems.address
            FROM systems
            JOIN system_factions
              ON system_factions.system_address = systems.address
            WHERE system_factions.faction_id = ANY($6)
              AND ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $1)
            "#,
            range,
            center[0],
            center[1],
            center[2],
            addresses,
            factions,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().filter_map(|row| row.address).collect())
    }

    /// Which factions are present in each of `addresses`, by address
    ///
    /// Kept to what was asked about, so the work grows with what is being
    /// looked at rather than with the table. A handful of addresses is a
    /// handful of index lookups; enough of them to be most of the sky is a
    /// scan of `system_factions`, which is what asking about most of the sky
    /// costs anyway.
    ///
    /// A row per membership rather than an array per system. Building the
    /// arrays in the database carries five times fewer rows and takes five
    /// times as long, the work being in the arrays rather than in the rows.
    ///
    /// Systems with nobody on record are simply absent, which is what the
    /// caller reads as nobody.
    async fn system_factions(
        db: &Database,
        addresses: &[i64],
    ) -> Result<HashMap<i64, Vec<i32>>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT system_address, faction_id
            FROM system_factions
            WHERE system_address = ANY($1)
            "#,
            addresses,
        )
        .fetch_all(&db.pool)
        .await?;

        let mut present: HashMap<i64, Vec<i32>> = HashMap::new();
        for row in rows {
            present.entry(row.system_address).or_default().push(row.faction_id);
        }

        Ok(present)
    }

    /// The systems at any of `addresses`
    ///
    /// One query for a set of them, since what asks is holding a list it
    /// already knows and wants all of it filled in at once.
    ///
    /// Addresses that match nothing are simply absent from the answer, and
    /// the order is the database's rather than the one asked in.
    pub async fn fetch_many(
        db: &Database,
        addresses: &[i64],
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
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
            WHERE address = ANY($1)
            "#,
            addresses,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
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
            .collect())
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

        Ok(rows
            .into_iter()
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
            .collect())
    }

    /// The systems heard from since `moment`, newest first
    ///
    /// What the map asks to draw the feed arriving. An hour of it touches about
    /// nine thousand systems scattered across the galaxy, so this is asked of
    /// the whole table rather than of a region: there is nowhere to look that
    /// the answer would be in.
    ///
    /// `most` bounds it, because the far end of the control putting this
    /// question is every system on record. Newest first so that what the bound
    /// drops is the oldest news rather than an arbitrary slice of it.
    pub async fn fetch_changed_since(
        db: &Database,
        moment: DateTime<Utc>,
        most: i64,
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
            WHERE systems.updated_at >= $1
            ORDER BY systems.updated_at DESC
            LIMIT $2
            "#,
            moment.naive_utc(),
            most,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
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
            .collect())
    }
}
