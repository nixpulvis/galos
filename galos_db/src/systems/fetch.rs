use super::{Economies, Survey, System};
use crate::{escaped, Database, Error};
use chrono::{DateTime, NaiveDateTime, Utc};
use elite_journal::prelude::*;
use geozero::wkb;
use std::collections::HashMap;

/// The surveys taken apart into a column each, as arrays go over the wire
///
/// Postgres takes a list of rows as one array per column and puts them back
/// together with `unnest`, there being no array of records to bind. Read back
/// in the order they are given here.
///
/// Naive times, `updated_at` being a `timestamp` without a zone and the two
/// having to compare.
type Spread = (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<NaiveDateTime>);

fn spread(surveyed: &[Survey]) -> Spread {
    let mut spread = Spread::default();
    for survey in surveyed {
        spread.0.push(survey.center[0]);
        spread.1.push(survey.center[1]);
        spread.2.push(survey.center[2]);
        spread.3.push(survey.range);
        spread.4.push(survey.at.naive_utc());
    }

    spread
}

/// Whether `at` stands within `range` light years of `center`
///
/// Compared as squares. The distance itself is never reported, so a square
/// root would be paid for on every system in a region and read by nobody.
fn within(at: &Coordinate, center: [f64; 3], range: f64) -> bool {
    let away = [at.x - center[0], at.y - center[1], at.z - center[2]];

    away.iter().map(|d| d * d).sum::<f64>() <= range * range
}

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
        let reaching = Self::reaches(db, &[row.address]).await?;

        Ok(System {
            reach: reaching.get(&row.address).copied(),
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
        let reaching = Self::reaches(db, &[row.address]).await?;

        Ok(System {
            reach: reaching.get(&row.address).copied(),
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

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut reaching = Self::reaches(db, &found).await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                reach: reaching.remove(&row.address),
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

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut reaching = Self::reaches(db, &found).await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                reach: reaching.remove(&row.address),
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

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut reaching = Self::reaches(db, &found).await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                reach: reaching.remove(&row.address),
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

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut reaching = Self::reaches(db, &found).await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                reach: reaching.remove(&row.address),
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
    ///
    /// `since` narrows it again, to what has been heard from since a moment.
    /// Narrowing only, as the two lists are: what it leaves out is what the
    /// filter excludes, and the region is what says how much sky is being
    /// asked about.
    ///
    /// `sizing` is how far from `center` a system's reach is worth knowing, in
    /// light years. Beyond it the systems still come back, and come back
    /// without a reach: the caller draws those at whatever it draws a system
    /// it cannot size, which past that distance is what they are drawn at
    /// anyway. [`None`] asks about all of them, which is what a caller that is
    /// not drawing a sky wants.
    ///
    /// It narrows the work rather than the answer, and it narrows the
    /// expensive half. What a system holds lives in three tables and is
    /// several rows deep in each, where the systems themselves are one row
    /// apiece; a region wide enough to be worth drawing holds far more systems
    /// than are near enough for their own size to show.
    ///
    /// Left to [`Self::fetch_many`] where a filter is admitting, that being a
    /// question about the handful of systems a filter named rather than about
    /// a region, and already the narrow one.
    ///
    /// `surveyed` is what the caller already holds, as [`Survey`]s, and what
    /// it holds is left out of the answer. A system is left out where any one
    /// of them reaches it and it has not changed since that one was taken, so
    /// a caller that has flown about is answered for the union of everywhere
    /// it has been rather than for the last place only. Empty asks for the
    /// whole region, which is what a caller holding nothing wants.
    ///
    /// One question rather than two. A caller standing still wants what has
    /// changed, and a caller that has moved wants the sky it has come to see;
    /// asked apart, a poll that only asked what had changed while the camera
    /// drifted would lose the systems it drifted onto for good, the next poll
    /// reaching back only as far as this one. Asked together the two are the
    /// same question, which is everything in range this caller cannot already
    /// answer for.
    ///
    /// A survey narrowed by `admitting` or `since` must not be handed back
    /// here. What came of one is the part of a region a filter admitted, and
    /// leaving a whole region out on the strength of it drops every system the
    /// filter turned away.
    ///
    /// Nothing within `sizing` of `center` is left out, whatever the surveys
    /// say. They answer for how far a system reaches only where they reached
    /// it from inside that same distance, so a system surveyed from further
    /// off came back without one; left out on the strength of that survey it
    /// would go on being drawn as a system of no known size for as long as it
    /// sat still, however near the caller came to it. The sky that near is a
    /// couple of thousand systems at the crowded end, which is what reading it
    /// again every time costs.
    pub async fn fetch_in_range_of_point(
        db: &Database,
        range: f64,
        center: [f64; 3],
        admitting: Option<(&[i32], &[i64])>,
        since: Option<DateTime<Utc>>,
        sizing: Option<f64>,
        surveyed: &[Survey],
    ) -> Result<Vec<Self>, Error> {
        let admitted = match (admitting, since) {
            (Some((factions, addresses)), Some(moment)) => Some(
                Self::admitted_in_range_since(
                    db, range, center, factions, addresses, moment,
                )
                .await?,
            ),
            (Some((factions, addresses)), None) => Some(
                Self::admitted_in_range(db, range, center, factions, addresses)
                    .await?,
            ),
            (None, Some(moment)) => {
                Some(Self::changed_in_range(db, range, center, moment).await?)
            }
            (None, None) => None,
        };
        if let Some(admitted) = admitted {
            return Self::fetch_many(db, &admitted).await;
        }

        let (xs, ys, zs, ranges, ats) = spread(surveyed);
        // Planned against what is bound rather than against parameters in
        // general. Postgres stops looking at the values after the fifth asking
        // of a prepared statement and settles on one plan for all of them,
        // which for this one runs at twice the time: the arrays are the shape
        // of the question, and a plan that cannot see whether there are none
        // of them or eight is planned for a question nobody puts.
        //
        // Set for the statement rather than for the connection. The pool is
        // shared with whatever else the caller is doing, and the sync writing
        // through it puts the same few upserts millions of times, which is the
        // case a held plan is for.
        //
        // The transaction is what `SET LOCAL` needs to be scoped by. Three
        // round trips against a query measured in seconds.
        let mut asking = db.pool.begin().await?;
        sqlx::query("SET LOCAL plan_cache_mode = force_custom_plan")
            .execute(&mut *asking)
            .await?;
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
              AND (
                ($10::float8 IS NOT NULL
                 AND ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $10))
                OR NOT EXISTS (
                  SELECT 1
                  FROM unnest($5::float8[], $6::float8[], $7::float8[],
                              $8::float8[], $9::timestamp[])
                      AS surveyed(x, y, z, range, at)
                  WHERE ST_3DDWithin(
                          ST_MakePoint(surveyed.x, surveyed.y, surveyed.z),
                          position, surveyed.range)
                    AND systems.updated_at <= surveyed.at
                )
              )
            "#,
            range,
            center[0],
            center[1],
            center[2],
            &xs,
            &ys,
            &zs,
            &ranges,
            &ats,
            sizing,
        )
        .fetch_all(&mut *asking)
        .await?;
        asking.commit().await?;

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut present = Self::system_factions(db, &found).await?;
        let mut reaching = match sizing {
            Some(sizing) => {
                let near: Vec<i64> = rows
                    .iter()
                    .filter(|row| {
                        row.position
                            .as_ref()
                            .and_then(|position| position.geometry.as_ref())
                            .is_some_and(|at| within(at, center, sizing))
                    })
                    .map(|row| row.address)
                    .collect();
                Self::reaches(db, &near).await?
            }
            None => Self::reaches(db, &found).await?,
        };

        Ok(rows
            .into_iter()
            .map(|row| System {
                reach: reaching.remove(&row.address),
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

    /// [`Self::admitted_in_range`], of those heard from since `moment`
    ///
    /// The stamp goes on both halves rather than around the two, so that each
    /// still drives from what it drove from before: put outside, it would be a
    /// filter over the union and leave the halves reaching for everything in
    /// range again.
    #[allow(clippy::too_many_arguments)]
    async fn admitted_in_range_since(
        db: &Database,
        range: f64,
        center: [f64; 3],
        factions: &[i32],
        addresses: &[i64],
        moment: DateTime<Utc>,
    ) -> Result<Vec<i64>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT address
            FROM systems
            WHERE ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $1)
              AND address = ANY($5)
              AND updated_at >= $7
            UNION
            SELECT systems.address
            FROM systems
            JOIN system_factions
              ON system_factions.system_address = systems.address
            WHERE system_factions.faction_id = ANY($6)
              AND ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $1)
              AND systems.updated_at >= $7
            "#,
            range,
            center[0],
            center[1],
            center[2],
            addresses,
            factions,
            moment.naive_utc(),
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().filter_map(|row| row.address).collect())
    }

    /// Which systems within `range` of `center` were heard from since `moment`
    ///
    /// What a question about time alone narrows a region to. No union, nothing
    /// being admitted by name or by faction: the stamp is the whole of what is
    /// asked, and it drives from the index on it.
    ///
    /// Unordered, as the other two are. What comes back is a set of addresses
    /// for [`Self::fetch_many`] to read rows for, and it reads them in whatever
    /// order it finds them, so sorting here is a sort nothing looks at.
    async fn changed_in_range(
        db: &Database,
        range: f64,
        center: [f64; 3],
        moment: DateTime<Utc>,
    ) -> Result<Vec<i64>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT address
            FROM systems
            WHERE ST_3DDWithin(ST_MakePoint($2, $3, $4), position, $1)
              AND updated_at >= $5
            "#,
            range,
            center[0],
            center[1],
            center[2],
            moment.naive_utc(),
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().map(|row| row.address).collect())
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

    /// How far each of `addresses` reaches from its arrival star, in metres
    ///
    /// The furthest thing on record, measured to the far side of what is drawn
    /// for it: how far from arrival the scan put it or the far end of its
    /// orbit, whichever is greater, with its own radius on top. A scan records
    /// where a thing stood on the day, so the orbit is what says how far it
    /// ever gets, and the recorded distance is what says how far its parent
    /// stands from the middle.
    ///
    /// The points a close pair goes round count as well. Nothing stands at
    /// one, but the pair rides its ellipse, and a pair scanned near periapsis
    /// says nothing about how far that ellipse reaches.
    ///
    /// Eccentricity is held short of one. What is recorded is a scan rather
    /// than a solution, and a parabola read literally reaches forever.
    ///
    /// The `299792458` is the metres in a light second, the distances from
    /// arrival being recorded in those and everything else in metres.
    ///
    /// Systems with nothing on record are simply absent, which is what the
    /// caller reads as not knowing.
    async fn reaches(
        db: &Database,
        addresses: &[i64],
    ) -> Result<HashMap<i64, f32>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT system_address AS "address!",
                   MAX(GREATEST(away, apoapsis) + radius) AS "reach!"
            FROM (
                SELECT system_address,
                       (COALESCE(distance_from_arrival, 0) * 299792458)::real
                           AS away,
                       (semi_major_axis
                           * (1 + LEAST(eccentricity, 0.99)))::real AS apoapsis,
                       radius
                FROM bodies
                WHERE system_address = ANY($1)
              UNION ALL
                SELECT system_address,
                       (distance_from_arrival_ls * 299792458)::real,
                       (COALESCE(semi_major_axis, 0)
                           * (1 + LEAST(COALESCE(eccentricity, 0), 0.99)))::real,
                       radius
                FROM stars
                WHERE system_address = ANY($1)
              UNION ALL
                SELECT system_address,
                       0::real,
                       (COALESCE(semi_major_axis, 0)
                           * (1 + LEAST(COALESCE(eccentricity, 0), 0.99)))::real,
                       0::real
                FROM barycenters
                WHERE system_address = ANY($1)
            ) reaching
            GROUP BY system_address
            "#,
            addresses,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().map(|row| (row.address, row.reach)).collect())
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

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut reaching = Self::reaches(db, &found).await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                reach: reaching.remove(&row.address),
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

        let found: Vec<i64> = rows.iter().map(|row| row.address).collect();
        let mut reaching = Self::reaches(db, &found).await?;

        Ok(rows
            .into_iter()
            .map(|row| System {
                reach: reaching.remove(&row.address),
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
