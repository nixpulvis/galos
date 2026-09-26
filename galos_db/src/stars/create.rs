use super::Star;
use crate::bodies::{ancestry, columns, Parent};
use crate::orbit;
use crate::Error;
use chrono::{DateTime, Utc};
use elite_journal::body::{Spin, Star as JournalStar};
use tracing::debug;

impl Star {
    /// `discovered_at` is worked out by the caller rather than read off the
    /// scan. A scan reporting the star undiscovered is itself the discovery,
    /// so the reading is the enclosing entry's timestamp, and only something
    /// holding that entry can say which that is.
    ///
    /// Answers the star on record, which is not always the one handed in:
    /// a system already holding a star of this name holds this star, and
    /// [`settle`] says which of the two records is kept.
    pub async fn from_journal(
        conn: &mut sqlx::PgConnection,
        timestamp: DateTime<Utc>,
        user: &str,
        star: &JournalStar,
        system_address: i64,
        discovered_at: Option<DateTime<Utc>>,
    ) -> Result<Star, Error> {
        // A name already answered to in this system is this star written
        // twice, and only one of the two records is kept.
        if let Some(kept) = settle(&mut *conn, system_address, star).await? {
            return Ok(kept);
        }

        // Kept where a scan names none, as a body's and a ring's are. A
        // primary star has no ancestor to name, and nothing tells that apart
        // from a scan that left the field out, so the two are stored the same
        // way and read back the same way.
        let parents = Parent::chain(&star.parents);
        let (parent_ids, parent_types) = columns(&parents);
        let parent_id = parent_ids.first().copied();
        let orbit = star.orbit.as_ref();

        let row = sqlx::query!(
            "
            INSERT INTO stars (
                system_address,
                id,
                name,
                parent_id,
                parent_ids,
                parent_types,
                updated_at,
                updated_by,

                absolute_magnitude,
                age_my,
                distance_from_arrival_ls,
                luminosity,
                star_class,
                stellar_mass,
                subclass,

                ascending_node,
                axial_tilt,
                eccentricity,
                mean_anomaly,
                orbital_inclination,
                orbital_period,
                periapsis,
                radius,
                rotation_period,
                semi_major_axis,
                temperature,

                was_mapped,
                discovered_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $3,
                parent_id = COALESCE($4, stars.parent_id),
                parent_ids = COALESCE($5, stars.parent_ids),
                parent_types = COALESCE($6, stars.parent_types),
                -- A message delivered late is still taken, for whatever it
                -- fills in below, and does not put the reading back in time.
                updated_at = GREATEST(stars.updated_at, $7),
                updated_by = CASE WHEN $7 >= stars.updated_at
                    THEN $8 ELSE stars.updated_by END,

                absolute_magnitude = $9,
                age_my = $10,
                distance_from_arrival_ls = $11,
                luminosity = $12,
                star_class = $13,
                stellar_mass = $14,
                subclass = $15,

                ascending_node = COALESCE($16, stars.ascending_node),
                axial_tilt = $17,
                eccentricity = COALESCE($18, stars.eccentricity),
                mean_anomaly = COALESCE($19, stars.mean_anomaly),
                orbital_inclination = COALESCE($20, stars.orbital_inclination),
                orbital_period = COALESCE($21, stars.orbital_period),
                periapsis = COALESCE($22, stars.periapsis),
                radius = $23,
                rotation_period = $24,
                semi_major_axis = COALESCE($25, stars.semi_major_axis),
                temperature = $26,

                -- Only ever up. A scan that finds a star already mapped is
                -- knowledge, and one that finds it unmapped is not evidence
                -- that it has stayed that way.
                was_mapped = stars.was_mapped OR $27,
                -- The earliest claim on record wins, and `LEAST` ignores a
                -- null, so a scan that says nothing about discovery leaves
                -- what is there alone.
                discovered_at = LEAST(stars.discovered_at, $28),
                -- `updated_at` says when the thing happened out in the galaxy,
                -- `received_at` says when the scan reached us, and only the
                -- second is any use to something following the feed.
                -- Unconditional, so a scan that changes nothing else still
                -- says it arrived. Said in UTC rather than left to the
                -- session's zone, because what reads it keeps its cursor in
                -- UTC and the two have to be the one clock.
                received_at = clock_timestamp() AT TIME ZONE 'utc'
            RETURNING *
            ",
            system_address,
            star.id,
            star.name,
            parent_id,
            (!parent_ids.is_empty()).then_some(&parent_ids[..]),
            (!parent_types.is_empty()).then_some(&parent_types[..]),
            timestamp.naive_utc(),
            user,
            star.absolute_magnitude,
            star.age_my,
            star.distance_from_arrival_ls,
            star.luminosity,
            star.star_class,
            star.stellar_mass,
            star.subclass,
            orbit.and_then(|orbit| orbit.ascending_node),
            star.spin.tilt,
            orbit.map(|orbit| orbit.eccentricity),
            orbit.and_then(|orbit| orbit.mean_anomaly),
            orbit.map(|orbit| orbit.orbital_inclination),
            orbit.map(|orbit| orbit.orbital_period),
            orbit.map(|orbit| orbit.periapsis),
            star.radius,
            star.spin.period,
            orbit.map(|orbit| orbit.semi_major_axis),
            star.temperature,
            star.discovery.mapped,
            discovered_at.map(|at| at.naive_utc()),
        )
        .fetch_one(&mut *conn)
        .await?;

        Ok(star!(row))
    }
}

/// Settle the name a star is coming in under against one already on record
///
/// Elite names a body uniquely within its system, so two stars of one
/// system under one name are one star written twice -- which is what
/// `UNIQUE (system_address, name)` on the table says. What the dump carries
/// is a pair of records differing in nothing but `bodyId` and
/// `distanceToArrival`.
///
/// The record kept is the nearer of the two to the arrival point, ties
/// broken by the lower id. That is the order
/// `galos_index::records::derive::arrival_class` picks the arrival star in and the
/// same comparison, so the row this keeps is the row the index derived from
/// a dump keeps, and the arrival class both read off it is the one star.
///
/// Answers the row on record where that record wins: the caller has nothing
/// left to write and that row is its answer. [`None`] where the write is to
/// go ahead, either because no other row answers to the name or because the
/// one that did has been dropped in favour of what is coming in.
///
/// # Under concurrent writers
///
/// The read is made safe by an advisory lock on the system and the name,
/// held for the rest of the transaction. Every write to `stars` comes
/// through here, and the key is the name the write carries, so any two
/// writes that could collide on that unique index wait on the same lock and
/// the second sees what the first committed. Without it two writers of one
/// new name both read nothing, both insert, and the unique index turns the
/// second away -- which costs the whole entry, this being one transaction.
///
/// The lock is taken in a statement of its own because a statement's
/// snapshot is taken before it runs: a lock acquired inside the same
/// statement as the read would leave the read looking at the galaxy as it
/// was before the writer ahead of it committed.
///
/// Transaction-scoped, so commit and rollback both release it and there is
/// no unlock to leak. A transaction writing several stars holds one per
/// name; the order they are taken in is the order the source lists them,
/// and two sources listing one system's stars in opposite orders deadlock,
/// which Postgres detects and reports as the refusal of one entry.
async fn settle(
    conn: &mut sqlx::PgConnection,
    system_address: i64,
    star: &JournalStar,
) -> Result<Option<Star>, Error> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, $2))",
        star.name,
        system_address,
    )
    .execute(&mut *conn)
    .await?;

    let twin = sqlx::query!(
        "
        SELECT *
        FROM stars
        WHERE system_address = $1 AND name = $2 AND id <> $3
        ",
        system_address,
        star.name,
        star.id,
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = twin else { return Ok(None) };

    // `total_cmp` and not `<`, so the rule is the one comparison
    // `arrival_class` makes rather than one that reads a pair of equal
    // distances differently.
    let on_record_wins = row
        .distance_from_arrival_ls
        .total_cmp(&star.distance_from_arrival_ls)
        .then(row.id.cmp(&star.id))
        .is_le();
    debug!(
        system = system_address,
        star = %star.name,
        kept = if on_record_wins { row.id } else { star.id },
        dropped = if on_record_wins { star.id } else { row.id },
        "one star written twice",
    );
    if on_record_wins {
        return Ok(Some(star!(row)));
    }

    // The row kept is the one coming in, and it is coming in under another
    // id, so the record it supersedes is deleted rather than updated. That
    // also clears the way for the insert below: a star renamed into a name
    // this system already holds would otherwise break the primary key
    // instead.
    sqlx::query!(
        "DELETE FROM stars WHERE system_address = $1 AND id = $2",
        system_address,
        row.id,
    )
    .execute(&mut *conn)
    .await?;
    Ok(None)
}
