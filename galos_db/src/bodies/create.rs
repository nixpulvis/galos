use super::{composition, Body, Parent, Surface};
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::body::{
    Body as JournalBody, Discovery, Material, Orbit, Spin,
};

impl Body {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        body: &JournalBody,
        system_address: i64,
    ) -> Result<Body, Error> {
        // A scan names each ancestor as a one entry map of kind to id,
        // nearest first. Kept in that order and whole, since the walk back to
        // the star is what places the body, and an ancestor that is not on
        // record is only skippable if what follows it is still known.
        let parents: Vec<Parent> = body
            .parents
            .iter()
            .filter_map(|parent| {
                let (ty, id) = parent.iter().next()?;
                Some(Parent { ty: Some(ty.clone()), id: *id })
            })
            .collect();
        let parent_types: Vec<String> = parents
            .iter()
            .map(|parent| parent.ty.clone().unwrap_or_default())
            .collect();
        let parent_ids: Vec<i16> =
            parents.iter().map(|parent| parent.id).collect();
        let parent_id = parent_ids.first().copied();

        let body_type = body.ty.as_ref().map(|ty| ty.to_string());

        // Everything a surface has goes in as nothing where there is none,
        // rather than as a default that would read as a measurement.
        let surface = body.surface.as_ref();
        let atmosphere_type =
            surface.map(|surface| surface.atmosphere_type.to_string());
        let surface_pressure = surface.map(|surface| surface.pressure);
        let crust = surface.map(|surface| &surface.composition);
        let composition_ice = crust.map(|crust| crust.ice);
        let composition_rock = crust.map(|crust| crust.rock);
        let composition_metal = crust.map(|crust| crust.metal);
        let landable = surface.map(|surface| surface.landable);
        let atmosphere = surface.and_then(|surface| surface.atmosphere.clone());
        let volcanism = surface.and_then(|surface| surface.volcanism.clone());
        let terraform_state =
            surface.and_then(|surface| surface.terraform_state.clone());

        let materials: Vec<Material> = surface
            .map(|surface| surface.materials.clone())
            .unwrap_or_default();
        let material_names: Vec<String> =
            materials.iter().map(|m| m.name.clone()).collect();
        let material_percents: Vec<f64> =
            materials.iter().map(|m| m.percent).collect();

        // The body and what it is made of go in together, so nothing reads a
        // body that is briefly made of nothing.
        let mut tx = db.pool.begin().await?;

        let row = sqlx::query!(
            "
            INSERT INTO bodies (
                name,
                id,
                parent_id,
                parent_ids,
                parent_types,
                system_address,
                updated_at,
                updated_by,

                body_type,
                distance_from_arrival,
                planet_class,
                tidal_lock,
                landable,
                terraform_state,
                atmosphere,
                atmosphere_type,
                volcanism,

                mass,
                radius,
                gravity,
                temperature,
                surface_pressure,
                composition_ice,
                composition_rock,
                composition_metal,
                semi_major_axis,
                eccentricity,
                orbital_inclination,
                periapsis,
                orbital_period,
                rotation_period,
                axial_tilt,
                ascending_node,
                mean_anomaly,

                was_mapped,
                was_discovered)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                COALESCE($12, false), COALESCE($13, false), $14, $15, $16, $17,
                $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33,
                $34, $35, $36)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $1,
                parent_id = $3,
                parent_ids = $4,
                parent_types = $5,
                updated_at = $7,
                updated_by = $8,

                body_type = COALESCE($9, bodies.body_type),
                distance_from_arrival = COALESCE($10, bodies.distance_from_arrival),
                planet_class = $11,
                tidal_lock = COALESCE($12, bodies.tidal_lock),
                landable = COALESCE($13, bodies.landable),
                terraform_state = COALESCE($14, bodies.terraform_state),
                atmosphere = COALESCE($15, bodies.atmosphere),
                atmosphere_type = COALESCE($16, bodies.atmosphere_type),
                volcanism = COALESCE($17, bodies.volcanism),

                mass = $18,
                radius = $19,
                gravity = $20,
                temperature = COALESCE($21, bodies.temperature),
                surface_pressure = COALESCE($22, bodies.surface_pressure),
                composition_ice = COALESCE($23, bodies.composition_ice),
                composition_rock = COALESCE($24, bodies.composition_rock),
                composition_metal = COALESCE($25, bodies.composition_metal),
                semi_major_axis = $26,
                eccentricity = $27,
                orbital_inclination = $28,
                periapsis = $29,
                orbital_period = $30,
                rotation_period = $31,
                axial_tilt = $32,
                ascending_node = $33,
                mean_anomaly = $34,

                was_mapped = $35,
                was_discovered = $36
            RETURNING *
            ",
            body.name,
            body.id,
            parent_id,
            &parent_ids,
            &parent_types,
            system_address,
            timestamp.naive_utc(),
            user,
            body_type,
            body.distance_from_arrival,
            body.planet_class,
            body.tidal_lock,
            landable,
            terraform_state,
            atmosphere,
            atmosphere_type,
            volcanism,
            body.mass,
            body.radius,
            body.gravity,
            body.temperature,
            surface_pressure,
            composition_ice,
            composition_rock,
            composition_metal,
            body.orbit.semi_major_axis,
            body.orbit.eccentricity,
            body.orbit.orbital_inclination,
            body.orbit.periapsis,
            body.orbit.orbital_period,
            body.spin.period,
            body.spin.tilt,
            body.orbit.ascending_node,
            body.orbit.mean_anomaly,
            body.discovery.mapped,
            body.discovery.discovered
        )
        .fetch_one(&mut *tx)
        .await?;

        // Only where the scan looked at a surface. One that did states the
        // whole of what the body is made of, so what it leaves out the body no
        // longer carries; one that did not says nothing on the subject, and a
        // gas giant and a basic scan both carry no materials for want of
        // having looked rather than for want of anything being there.
        if body.surface.is_some() {
            sqlx::query!(
                "DELETE FROM body_materials WHERE system_address = $1 AND body_id = $2",
                system_address,
                body.id,
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query!(
                // The rows this writes were just cleared, so nothing here can
                // meet one already stored. `DISTINCT ON` answers for the other
                // way a conflict arises, which is one scan naming a material
                // twice: two rows conflicting inside a single statement is an
                // error rather than something `ON CONFLICT` can settle. The
                // first reading wins.
                "
                INSERT INTO body_materials (system_address, body_id, name, percent)
                SELECT DISTINCT ON (name) $1, $2, name, percent
                FROM UNNEST($3::varchar[], $4::double precision[]) AS m(name, percent)
                ORDER BY name
                ",
                system_address,
                body.id,
                &material_names,
                &material_percents,
            )
            .execute(&mut *tx)
            .await?;
        }

        // Read back rather than handed on from the scan. A scan that did not
        // look at a surface leaves the stored materials where they are, so what
        // is on record is not what arrived, and every other field below comes
        // off the row for the same reason.
        let materials = sqlx::query!(
            "
            SELECT name, percent
            FROM body_materials
            WHERE system_address = $1 AND body_id = $2
            ORDER BY name
            ",
            system_address,
            body.id,
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| Material { name: row.name, percent: row.percent })
        .collect::<Vec<_>>();

        tx.commit().await?;

        Ok(Body {
            system_address: row.system_address,
            id: row.id,
            parents,
            name: row.name,
            body_type: row.body_type.map(|ty| ty.as_str().into()),
            distance_from_arrival: row.distance_from_arrival,
            planet_class: row.planet_class,
            tidal_lock: row.tidal_lock,
            surface: Surface::read(
                row.atmosphere_type,
                row.surface_pressure,
                composition(
                    row.composition_ice,
                    row.composition_rock,
                    row.composition_metal,
                ),
                row.landable,
                row.atmosphere,
                row.volcanism,
                row.terraform_state,
                materials,
            ),
            mass: row.mass,
            radius: row.radius,
            gravity: row.gravity,
            temperature: row.temperature,

            orbit: Orbit {
                semi_major_axis: row.semi_major_axis,
                eccentricity: row.eccentricity,
                orbital_inclination: row.orbital_inclination,
                periapsis: row.periapsis,
                orbital_period: row.orbital_period,
                ascending_node: row.ascending_node,
                mean_anomaly: row.mean_anomaly,
            },
            spin: Spin { period: row.rotation_period, tilt: row.axial_tilt },
            discovery: Discovery {
                discovered: row.was_discovered,
                mapped: row.was_mapped,
            },
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }
}
