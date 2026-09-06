use super::{Faction, SystemFaction};
use crate::{Database, Error};
use elite_journal::{faction::State as JournalState, prelude::*};

/// What the user typed, as a `LIKE` pattern matching those letters
///
/// A name is a thing the user is part way through typing, so `%` and `_` in it
/// are characters they typed rather than wildcards they meant. Both mean
/// something to `LIKE` and nothing to whoever typed them, so they are held out
/// at the pattern's own escape character.
///
/// The escape itself goes first, or the backslash put in front of the other
/// two would go on to be read as an escape in its own right.
fn escaped(query: &str) -> String {
    query.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_")
}

impl Faction {
    pub async fn fetch(db: &Database, id: i32) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM factions
            WHERE id = $1
            ",
            id
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Faction { id: row.id, name: row.name })
    }

    pub async fn fetch_by_name(
        db: &Database,
        name: &str,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM factions
            WHERE lower(name) = $1
            ",
            name.to_lowercase()
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Faction { id: row.id, name: row.name })
    }

    /// The factions with any of `ids`
    ///
    /// One query for a set of them, since what asks is holding a system's
    /// whole list and wants all of it named at once.
    ///
    /// Ids that match nothing are simply absent from the answer, so the
    /// caller pairs by id rather than by position.
    pub async fn fetch_many(
        db: &Database,
        ids: &[i32],
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT id, name
            FROM factions
            WHERE id = ANY($1)
            ",
            ids
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Faction { id: row.id, name: row.name })
            .collect())
    }

    /// The factions whose names hold `query`, best first
    ///
    /// What a field asks where the user is part way through typing a name and
    /// wants to be shown which factions they might mean.
    ///
    /// `query` is read as letters rather than as a pattern, since a name is a
    /// thing the user is halfway through typing and `%` and `_` in it are
    /// characters they typed, which `escaped` takes them at their word for.
    /// That is the difference between this and [`Faction::fetch_like_name`],
    /// which takes a pattern whole from whoever wrote it, and answers with
    /// however many match.
    ///
    /// Ordered so that the `limit` keeps the rows worth keeping: the name
    /// spelled out in full, then names that start with the query, then the
    /// rest by name. Someone typing `dukes` means The Dukes of Mikunn before
    /// they mean Grand Duke Enterprise.
    ///
    /// Bounded because it has to be. A query of a letter matches most of the
    /// factions on record, `%a%` reaching four in five of the twenty-odd
    /// thousand held, and a list nobody can read to the end of is no more use
    /// for being complete.
    ///
    /// A share rather than a count of them. The ingest adds factions for as
    /// long as it runs, so a number written here is right on the day and
    /// drifts from then on, where what the bound is argued from is the share
    /// and that holds.
    pub async fn search_by_name(
        db: &Database,
        query: &str,
        limit: i64,
    ) -> Result<Vec<Self>, Error> {
        let query = escaped(query);
        let rows = sqlx::query!(
            r#"
            SELECT id, name
            FROM factions
            WHERE name ILIKE $1
            ORDER BY (name ILIKE $2) DESC, (name ILIKE $3) DESC, name
            LIMIT $4
            "#,
            format!("%{query}%"),
            query,
            format!("{query}%"),
            limit,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Faction { id: row.id, name: row.name })
            .collect())
    }

    pub async fn fetch_like_name(
        db: &Database,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT *
            FROM factions
            WHERE name ILIKE $1
            ORDER BY name
            "#,
            name
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Faction { id: row.id, name: row.name })
            .collect())
    }
}

impl SystemFaction {
    pub async fn fetch(
        db: &Database,
        address: i64,
        id: u32,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            r#"
            SELECT
                system_address,
                faction_id,
                name,
                state AS "state: JournalState",
                influence,
                happiness AS "happiness: Happiness",
                government AS "government: Government",
                allegiance AS "allegiance: Allegiance",
                updated_at
            FROM system_factions
            JOIN factions ON faction_id = id
            WHERE system_address = $1 AND faction_id = $2
            ORDER BY influence DESC
            "#,
            address as i64,
            id as i32
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(SystemFaction {
            system_address: row.system_address,
            faction_id: row.faction_id as u32,
            state: row.state,
            influence: row.influence,
            happiness: row.happiness,
            updated_at: row.updated_at.and_utc(),
        })
    }

    pub async fn fetch_all(
        db: &Database,
        address: Option<i64>,
    ) -> Result<Vec<(String, Self)>, Error> {
        if let Some(address) = address {
            let rows = sqlx::query!(
                r#"
            SELECT
                system_address,
                faction_id,
                name,
                state AS "state: JournalState",
                influence,
                happiness AS "happiness: Happiness",
                government AS "government: Government",
                allegiance AS "allegiance: Allegiance",
                updated_at
            FROM system_factions
            JOIN factions ON faction_id = id
            WHERE system_address = $1
            ORDER BY influence DESC
            "#,
                address as i64
            )
            .fetch_all(&db.pool)
            .await?;

            Ok(rows
                .into_iter()
                .map(|row| {
                    (
                        row.name,
                        SystemFaction {
                            system_address: row.system_address,
                            faction_id: row.faction_id as u32,
                            state: row.state,
                            influence: row.influence,
                            happiness: row.happiness,
                            updated_at: row.updated_at.and_utc(),
                        },
                    )
                })
                .collect())
        } else {
            let rows = sqlx::query!(
                r#"
            SELECT
                system_address,
                faction_id,
                name,
                state AS "state: JournalState",
                influence,
                happiness AS "happiness: Happiness",
                government AS "government: Government",
                allegiance AS "allegiance: Allegiance",
                updated_at
            FROM system_factions
            JOIN factions on faction_id = id
            ORDER BY influence DESC
            "#
            )
            .fetch_all(&db.pool)
            .await?;

            Ok(rows
                .into_iter()
                .map(|row| {
                    (
                        row.name,
                        SystemFaction {
                            system_address: row.system_address,
                            faction_id: row.faction_id as u32,
                            state: row.state,
                            influence: row.influence,
                            happiness: row.happiness,
                            updated_at: row.updated_at.and_utc(),
                        },
                    )
                })
                .collect())
        }
    }
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
    /// so a search for a literal one would answer with factions that have
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
