//! A body within a star system
use chrono::{DateTime, Utc};
use elite_journal::body::{
    AtmosphereType, BodyType, Composition, Discovery, Material, Orbit, Spin,
};
use std::collections::BTreeMap as Map;

/// Clone because the map carries one into a component and into whatever
/// panel is describing it, and a body outlives the query it came back in.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub system_address: i64,
    pub id: i16,
    /// Every ancestor the scan named, nearest first
    pub parents: Vec<Parent>,
    pub name: String,
    pub body_type: Option<BodyType>,
    /// How far the body is from the system's arrival star, in light seconds
    ///
    /// The one number that places a body without resolving its ancestry.
    pub distance_from_arrival: Option<f32>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

    pub planet_class: String,
    pub tidal_lock: bool,

    pub mass: f32,
    pub radius: f32,

    /// Measured at the cloud tops where there is no surface, which is why
    /// these are not part of [`Surface`]
    pub gravity: f32,
    pub temperature: Option<f32>,

    /// [`None`] for a gas giant, which has no surface to record
    pub surface: Option<Surface>,

    /// The path the body takes around its nearest ancestor
    pub orbit: Orbit,
    pub spin: Spin,
    pub discovery: Discovery,
}

impl Eq for Body {}

impl Body {
    /// The nearest ancestor, which is what the body's orbit is measured about
    pub fn parent_id(&self) -> Option<i16> {
        self.parents.first().map(|parent| parent.id)
    }
}

mod create;
mod fetch;

/// One ancestor of a body, as the scan named it
///
/// `ty` is left as it arrived rather than read into [`BodyType`], since an
/// unfamiliar one would otherwise drop an ancestor out of the middle of a
/// chain and shift everything above it down. It is [`None`] for a body stored
/// before the kinds were kept, which recorded the nearest ancestor's id alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parent {
    pub ty: Option<String>,
    pub id: i16,
}

impl Parent {
    /// Whether this ancestor is a barycenter, which is stored apart from bodies
    pub fn is_barycenter(&self) -> bool {
        self.ty.as_deref() == Some("Null")
    }

    /// The ancestry a scan named, nearest first
    ///
    /// A scan writes each ancestor as a one entry map of kind to id. Kept in
    /// that order and whole, since the walk back to the star is what places
    /// the thing, and an ancestor that is not on record can only be stepped
    /// over if what follows it is still known.
    pub(crate) fn chain(named: &[Map<String, i16>]) -> Vec<Self> {
        named
            .iter()
            .filter_map(|parent| {
                let (ty, id) = parent.iter().next()?;
                Some(Parent { ty: Some(ty.clone()), id: *id })
            })
            .collect()
    }

    /// The chain a pair of stored arrays holds, nearest first
    ///
    /// The ids are what a chain is walked by. The kinds went unrecorded until
    /// they were stored alongside, so a row may have the one without the
    /// other, and the ids are what decides how long the chain is.
    pub(crate) fn rows(
        ids: Option<Vec<i16>>,
        types: Option<Vec<String>>,
    ) -> Vec<Self> {
        let types = types.unwrap_or_default();
        ids.unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(depth, id)| Parent { ty: types.get(depth).cloned(), id })
            .collect()
    }

    /// The kinds and the ids as two arrays, which is how they are stored
    pub(crate) fn columns(chain: &[Self]) -> (Vec<i16>, Vec<String>) {
        (
            chain.iter().map(|parent| parent.id).collect(),
            chain
                .iter()
                .map(|parent| parent.ty.clone().unwrap_or_default())
                .collect(),
        )
    }
}

/// What a body with a surface has
///
/// Apart from [`elite_journal::body::Surface`] only in that the composition is
/// optional: a body stored before the fractions were kept has a surface and no
/// reading of what it is made of.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub atmosphere_type: AtmosphereType,
    pub pressure: f32,
    pub composition: Option<Composition>,

    pub landable: bool,
    pub atmosphere: Option<String>,
    pub volcanism: Option<String>,
    pub terraform_state: Option<String>,
    pub materials: Vec<Material>,
}

impl Surface {
    /// The atmosphere type and the pressure are what say there is a surface
    /// here at all. The rest may be absent from one that there is.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn read(
        atmosphere_type: Option<String>,
        pressure: Option<f32>,
        composition: Option<Composition>,
        landable: bool,
        atmosphere: Option<String>,
        volcanism: Option<String>,
        terraform_state: Option<String>,
        materials: Vec<Material>,
    ) -> Option<Self> {
        Some(Self {
            atmosphere_type: AtmosphereType::from(atmosphere_type?.as_str()),
            pressure: pressure?,
            composition,
            landable,
            atmosphere,
            volcanism,
            terraform_state,
            materials,
        })
    }
}

/// The three fractions a crust is described by, which are stored together
pub(crate) fn composition(
    ice: Option<f32>,
    rock: Option<f32>,
    metal: Option<f32>,
) -> Option<Composition> {
    Some(Composition { ice: ice?, rock: rock?, metal: metal? })
}
