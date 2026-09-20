use super::{Economies, Landed, System};
use crate::factions::{Conflict, Faction, SystemFaction};
use crate::Error;
use chrono::{DateTime, Utc};
use elite_journal::{prelude::*, system::System as JournalSystem};
use galos_index::{procedural, SystemName, SystemReport};
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
    ///
    /// Returns what the write did to the row; see [`Landed`].
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        conn: &mut sqlx::PgConnection,
        address: i64,
        name: &SystemName,
        position: Option<Coordinate>,
        primary_star_class: Option<String>,
        population: Option<u64>,
        security: Option<Security>,
        government: Option<Government>,
        allegiance: Option<Allegiance>,
        economies: Option<Economies>,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<Landed, Error> {
        // The name is written down only where the address does not spell
        // it. `procedural::spells` asks exactly what the index asks before
        // writing a name into its own table, and for 97.3 % of a galaxy
        // the answer is that the primary key already says it -- so what
        // the column holds is the exceptions and a null means "read it off
        // the address", which `System::name_of` is the one place that does.
        //
        // Weighed by the stamps like every other column: a null wins where
        // this reading wins, and a name that disagrees with the arithmetic
        // fills the column back in. Which is the overlay the index keeps
        // too -- a name somebody gave a system arrives after the name the
        // galaxy spelled it, and is the one to keep.
        let stored =
            (!procedural::spells(address, name)).then(|| name.as_str());

        let did = sqlx::query!(
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
            VALUES ($1, $2, $3, $4::geometry, $5, $6,
                $7, $8, $9, $10, $11, $12)
            ON CONFLICT (address)
            DO UPDATE SET
                name = CASE WHEN $11 >= systems.updated_at
                    THEN $2 ELSE systems.name END,
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
                    THEN $12 ELSE systems.updated_by END,
                -- `updated_at` says when the thing happened out in the galaxy,
                -- `received_at` says when the report reached us, and only the
                -- second is any use to something following the feed.
                -- Unconditional, so a report that changes nothing else still
                -- says it arrived. Said in UTC rather than left to the
                -- session's zone, because what reads it keeps its cursor in
                -- UTC and the two have to be the one clock.
                received_at = clock_timestamp() AT TIME ZONE 'utc'
            RETURNING
                -- Did this statement insert the row? The conflict path
                -- locks the row it is about to update and the new version
                -- carries that lock, so an update returns a live xmax and
                -- an insert returns 0.
                (xmax = 0) AS "inserted!",
                -- Did this reading win? `updated_at` above is
                -- `GREATEST(systems.updated_at, $11)`, which equals `$11`
                -- exactly when `$11 >= updated_at` -- the condition every
                -- `CASE` arm uses.
                (updated_at = $11) AS "took!"
            "#,
            address as i64,
            stored,
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
        .fetch_one(&mut *conn)
        .await?;

        Self::adopt_waiting_markets(
            &mut *conn, address, name, updated_at, updated_by,
        )
        .await?;

        Ok(Landed::of(did.inserted, did.took))
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
    /// Both names are [`SystemName`]s — this one's and `markets.system_name`,
    /// which `markets_system_name_uppercase` holds to the same spelling — so
    /// the comparison is a comparison rather than a fold of the parameter on
    /// every system write.
    async fn adopt_waiting_markets(
        conn: &mut sqlx::PgConnection,
        address: i64,
        name: &SystemName,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<(), Error> {
        // This runs on every system write, including the inner loop of the
        // bulk importers, and almost always there is nothing waiting. Ask
        // the partial index before writing two statements for no reason.
        let waiting = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM markets
                 WHERE system_address IS NULL AND system_name = $1
            ) AS "waiting!"
            "#,
            name.as_str(),
        )
        .fetch_one(&mut *conn)
        .await?;

        if !waiting {
            return Ok(());
        }

        sqlx::query!(
            r#"
            INSERT INTO stations (system_address, name, updated_at, updated_by)
            SELECT $1, m.station_name, $3, $4
              FROM markets m
             WHERE m.system_address IS NULL AND m.system_name = $2
            ON CONFLICT (system_address, name) DO NOTHING
            "#,
            address,
            name.as_str(),
            updated_at.naive_utc(),
            updated_by,
        )
        .execute(&mut *conn)
        .await?;

        sqlx::query!(
            r#"
            UPDATE markets SET system_address = $1
             WHERE system_address IS NULL AND system_name = $2
            "#,
            address,
            name.as_str(),
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    /// Write what a report says about a system.
    ///
    /// The one way in for everything above body level. `galos db ingest`
    /// hands this whatever [`SystemReport::of`](galos_index::SystemReport::of)
    /// made of an event and whatever a published dump gave it, so the
    /// fifteen events that name a system reach Postgres through one call
    /// rather than through a call apiece.
    ///
    /// A report is written for what it says, part by part, and no field is
    /// read as evidence about another:
    ///
    /// - Where it names the system, [`Self::create`] writes the name, the
    ///   place and the politics. Every political column there is
    ///   `COALESCE`d, so a report stating none leaves what stands.
    /// - Where it carries body counts, [`Self::set_body_counts`] writes
    ///   them — deliberately not stamp-guarded, for the reason stated
    ///   there. Asked by address: `create` has left the row wherever there
    ///   was a name, and naming it again would weigh the name and the
    ///   place against the stamps under a second copy of the rule.
    ///
    /// A dump's row says both, so it costs two statements where it cost
    /// one. `galos_db::record` runs the pair inside the one transaction,
    /// so a system cannot land with its counts and without its politics.
    ///
    /// A report that names no system writes nothing and is not an error. The
    /// `name` column does take a null, but a null there says the address
    /// spells the name rather than that nothing has named the system; the
    /// arrival that had to come first is what writes that row, and where it
    /// has not, the foreign key onto `systems` is what says so. The index's
    /// half of the program keeps such a report — a position now is a
    /// position for whatever names the place later — and the two agree on
    /// everything either of them publishes.
    ///
    /// Returns the wider of what the two did, a run counting systems and
    /// not statements: [`Landed::New`] where either made the row, else
    /// [`Landed::Updated`] where either moved it, else [`Landed::Stale`].
    /// [`None`] where no row was written: a report naming no system, or a
    /// count for a system nothing has named.
    pub async fn report(
        conn: &mut sqlx::PgConnection,
        report: &SystemReport,
        by: &str,
    ) -> Result<Option<Landed>, Error> {
        let politics = match report.name.as_ref() {
            Some(name) => Some(
                Self::create(
                    &mut *conn,
                    report.address,
                    name,
                    report.position,
                    report.star_class.clone(),
                    report.population,
                    report.security,
                    report.government,
                    report.allegiance,
                    Economies::new(
                        report.primary_economy,
                        report.secondary_economy,
                    ),
                    report.at,
                    by,
                )
                .await?,
            ),
            None => None,
        };

        let counts = match report.body_count {
            Some(bodies) => {
                Self::set_body_counts(
                    &mut *conn,
                    report.address,
                    None,
                    report.position,
                    bodies,
                    report.non_body_count,
                    report.at,
                    by,
                )
                .await?
            }
            None => None,
        };

        Ok(Landed::widest(politics, counts))
    }

    /// Write a system as an arrival event states it, factions and all.
    ///
    /// [`Self::create`] for the columns — one copy of the merge rule, not
    /// two — and then the rows only a journal ever carries. A faction and a
    /// conflict are rows of their own, each with a stamp of its own and its
    /// own say in whether a message is worth taking, which is why they are
    /// asked whatever became of the system's row above.
    ///
    /// Drops what the write did: `galos_db::record` has already written
    /// this system's row from [`Self::report`], so that is the landing a
    /// run counts and this second write of it is always an update.
    pub async fn from_journal(
        conn: &mut sqlx::PgConnection,
        timestamp: DateTime<Utc>,
        user: &str,
        system: &JournalSystem,
    ) -> Result<(), Error> {
        Self::create(
            &mut *conn,
            system.address,
            &SystemName::new(system.name.clone()),
            system.pos,
            None,
            system.population,
            system.security,
            system.government,
            system.allegiance,
            Economies::new(system.economy, system.second_economy),
            timestamp,
            user,
        )
        .await?;
        Self::factions(&mut *conn, system, timestamp).await
    }

    /// The rows only an arrival ever carries: who is present and who is at
    /// war.
    ///
    /// Apart from [`Self::report`] because they are apart from a system's own
    /// columns in every other way. A faction and a conflict are rows of their
    /// own, each with a stamp of its own and its own say in whether a message
    /// is worth taking, so they are asked whatever became of the system's row
    /// — and nothing else here derives them: a faction's numeric id is this
    /// crate's, minted when the row is first written, and the index publishes
    /// no faction it has not been handed one for.
    pub async fn factions(
        conn: &mut sqlx::PgConnection,
        system: &JournalSystem,
        timestamp: DateTime<Utc>,
    ) -> Result<(), Error> {
        for faction in &system.factions {
            let faction_id =
                Faction::create(&mut *conn, &faction.name).await?.id;
            SystemFaction::from_journal(
                &mut *conn,
                system.address,
                faction_id as u32,
                faction,
                timestamp,
            )
            .await?;
        }

        for conflict in &system.conflicts {
            Conflict::from_journal(
                &mut *conn,
                system.address,
                conflict,
                timestamp,
            )
            .await?;
        }

        Ok(())
    }

    /// Record how many bodies a system holds
    ///
    /// Three events report this and they are reporting the same number, so
    /// they arrive here together. `non_body_count` is the belts and rings,
    /// which only the honk counts; the others pass [`None`] and leave
    /// whatever is there alone.
    ///
    /// A named system need not be on record: a name and a position are
    /// enough to write the row the count belongs to. [`Self::report`] names
    /// nothing here, [`Self::create`] having left that row already, and a
    /// nav beacon names only an address as the game writes it — so under
    /// either the count is set on the system if it is there and dropped if
    /// it is not. The arrival that had to come first is what writes it.
    ///
    /// Unlike [`System::create`] this does not refuse an older message. A
    /// count does not go stale -- a system does not gain or lose bodies --
    /// and the timestamp guard would throw nearly all of them away, since a
    /// system busy enough to be honked at is busy enough to have been written
    /// more recently by something else. What an older message does not do is
    /// put the system's reading back to when it was sent.
    ///
    /// So this returns [`Landed::New`] or [`Landed::Updated`] and never
    /// [`Landed::Stale`]: the count is written whatever the stamps say.
    /// [`None`] means nothing was written, which is a count for a system
    /// nothing has named.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_body_counts(
        conn: &mut sqlx::PgConnection,
        address: i64,
        name: Option<&SystemName>,
        position: Option<Coordinate>,
        body_count: i32,
        non_body_count: Option<i32>,
        updated_at: DateTime<Utc>,
        updated_by: &str,
    ) -> Result<Option<Landed>, Error> {
        let Some(name) = name else {
            let done = sqlx::query!(
                r#"
                UPDATE systems SET
                    position = COALESCE($2, systems.position),
                    body_count = $3,
                    non_body_count =
                        COALESCE($4, systems.non_body_count),
                    updated_at = GREATEST(systems.updated_at, $5),
                    updated_by = CASE WHEN $5 >= systems.updated_at
                        THEN $6 ELSE systems.updated_by END,
                    received_at = clock_timestamp() AT TIME ZONE 'utc'
                WHERE address = $1
                "#,
                address,
                position.map(|p| wkb::Encode(p)) as _,
                body_count,
                non_body_count,
                updated_at.naive_utc(),
                updated_by,
            )
            .execute(&mut *conn)
            .await?;

            if done.rows_affected() == 0 {
                tracing::debug!(
                    address,
                    "body counts for a system nothing has named",
                );
                return Ok(None);
            }

            return Ok(Some(Landed::Updated));
        };

        // The same rule as [`Self::create`], and it has to be: the two
        // write one row, and a name the address spells is not written down
        // by either of them.
        let stored =
            (!procedural::spells(address, name)).then(|| name.as_str());

        let did = sqlx::query!(
            r#"
            INSERT INTO systems
                (address,
                 name,
                 position,
                 body_count,
                 non_body_count,
                 updated_at,
                 updated_by)
            VALUES ($1, $2, $3::geometry, $4, $5, $6, $7)
            ON CONFLICT (address)
            DO UPDATE SET
                name = CASE WHEN $6 >= systems.updated_at
                    THEN $2 ELSE systems.name END,
                -- Weighed by the stamps as `create` weighs it, the two
                -- statements being one rule about where a system is. The
                -- counts below are the exception and say why.
                position = CASE WHEN $6 >= systems.updated_at
                    THEN COALESCE($3, systems.position)
                    ELSE COALESCE(systems.position, $3) END,
                body_count = $4,
                non_body_count =
                    COALESCE($5, systems.non_body_count),
                updated_at = GREATEST(systems.updated_at, $6),
                updated_by = CASE WHEN $6 >= systems.updated_at
                    THEN $7 ELSE systems.updated_by END,
                received_at = clock_timestamp() AT TIME ZONE 'utc'
            RETURNING (xmax = 0) AS "inserted!"
            "#,
            address,
            stored,
            position.map(|p| wkb::Encode(p)) as _,
            body_count,
            non_body_count,
            updated_at.naive_utc(),
            updated_by,
        )
        .fetch_one(&mut *conn)
        .await?;

        Self::adopt_waiting_markets(
            &mut *conn, address, name, updated_at, updated_by,
        )
        .await?;

        Ok(Some(Landed::of(did.inserted, true)))
    }

    /// Record the class of the star a ship arrives at.
    ///
    /// What burns at the middle of a system is worth a column of its own
    /// even though the `stars` rows say it too: a system nobody has scanned
    /// still has one, off a plotted route, and it is what the index falls
    /// back to for a magnitude and a temperature. Until now a route was the
    /// only thing that ever wrote it, so a system somebody had actually
    /// *been* to and scanned was left with the column empty and the answer
    /// sitting in `stars` where nothing looking for a class thought to
    /// look.
    ///
    /// The caller decides what "arrives at" means — a scan of a star zero
    /// light seconds out — since that is a fact about the scan rather than
    /// about the row.
    ///
    /// Nothing else on the row moves, the stamps included. The scan that
    /// carries this has already written the system through
    /// `ensure_system`, so the pass that publishes it has already been told
    /// the system changed, and a class is not a reason for the map to
    /// redraw a system as freshly reported. A write is skipped where the
    /// column already says this, which on a feed reporting the same system
    /// over and over is nearly all of them.
    pub async fn set_primary_star_class(
        conn: &mut sqlx::PgConnection,
        address: i64,
        class: &str,
    ) -> Result<bool, Error> {
        let done = sqlx::query!(
            r#"
            UPDATE systems SET primary_star_class = $2
            WHERE address = $1 AND primary_star_class IS DISTINCT FROM $2
            "#,
            address,
            class,
        )
        .execute(&mut *conn)
        .await?;

        Ok(done.rows_affected() > 0)
    }
}
