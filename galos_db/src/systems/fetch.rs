use super::System;
use crate::{Database, Error};
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
            primary_economy: row.primary_economy,
            secondary_economy: row.secondary_economy,
            factions: row.factions,
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
            primary_economy: row.primary_economy,
            secondary_economy: row.secondary_economy,
            factions: row.factions,
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
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                factions: row.factions,
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
    /// of the systems on record, `%a%` reaching 180,000 of the 284,000 held
    /// today, and a list nobody can read to the end of is no more use for
    /// being complete.
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
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                factions: row.factions,
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
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                factions: row.factions,
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
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                factions: row.factions,
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
    /// Who is present in each of them is asked separately, by
    /// [`Self::system_factions`], and joined up here. Asked of every row in the
    /// same query it is a subquery run once per system: over a hundred
    /// thousand systems that is six parts in seven of the whole query, spent
    /// discovering that all but three in a hundred of them have nobody on
    /// record at all.
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
            .collect())
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
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                factions: row.factions,
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
                primary_economy: row.primary_economy,
                secondary_economy: row.secondary_economy,
                factions: row.factions,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
            })
            .collect())
    }
}

/// What the user typed, as a `LIKE` pattern matching those letters
///
/// `%` and `_` mean something to `LIKE` and nothing to whoever typed them, so
/// they are held out at the pattern's own escape character. The escape itself
/// goes first, or escaping the other two would go on to be read as an escape
/// in its own right.
fn escaped(query: &str) -> String {
    query.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name with nothing special in it is left as it is
    #[test]
    fn a_plain_name_is_left_alone() {
        assert_eq!(escaped("Col 285 Sector"), "Col 285 Sector");
    }

    /// The two characters `LIKE` reads are held out
    ///
    /// A user typing either means the character. Left as they are, `%` would
    /// match the rest of every name on record and `_` any character at all,
    /// so a search for a literal one would answer with systems that have
    /// nothing to do with it.
    #[test]
    fn the_wildcards_are_held_out() {
        assert_eq!(escaped("100%"), r"100\%");
        assert_eq!(escaped("a_b"), r"a\_b");
    }

    /// The escape character is held out first
    ///
    /// Or the backslash put in front of a `%` would itself be escaped
    /// afterwards, leaving `\\%`: a literal backslash followed by a wildcard,
    /// which is the wildcard the escaping was there to take away.
    #[test]
    fn the_escape_is_held_out_before_what_it_escapes() {
        assert_eq!(escaped(r"a\%b"), r"a\\\%b");
    }
}
