//! What a cell carries about the systems anybody lives in.
//!
//! [`Aggregate`](crate::core::aggregate::Aggregate) stands for every system in
//! a subtree, and its two weightings are the stellar ones: flux for the glow,
//! count for the density. Neither answers where the *inhabited* systems sit,
//! and the two are nothing alike — one 256 ly cell holds a few thousand
//! governed systems among some hundred thousand neighbours, so a political
//! field laid at the count-weighted centroid with the count-weighted spread
//! draws the colonized filaments as a blob over the whole cell and loses the
//! shape that is the picture.
//!
//! So this is a third weighting, kept beside the other two rather than folded
//! into them: **one unit of weight for every system somebody lives in, and
//! none for the rest**. The centroid is then where the colonies are and the
//! spread is how far they reach, both of which are what a political splat is
//! drawn from. Inhabited is exactly `population > 0` and is asked per system,
//! never reconstructed from a sum — a summed population says somebody lives
//! under a cell and never how many systems do.
//!
//! The histograms ride here rather than in their own record because they
//! have the same support: only a system on the populated table has an
//! allegiance, a government, a security rating, an economy, a state or a
//! power to count. They are counts and not fractions, because a count
//! composes exactly and a `u8` share of a total does not — merging two shares
//! needs their weights back, and the rounding accumulates through every level
//! of the rollup until [`Inhabited::remove`] stops being the inverse of
//! [`Inhabited::merge`].
//!
//! What the counts are *worth* is not stored. A view that wants unaligned
//! systems to weigh less than aligned ones applies its gain where it draws,
//! which keeps the gain a tunable rather than a format decision and keeps a
//! residual correct under any gain: bake a weight into the stored sum and a
//! residual taken under one gain is wrong under another.
//!
//! Derived rather than published. [`Inhabitance::of`] rolls the resident
//! `populated.bin` up a tree a reader already holds, which is the whole
//! column for the price of one pass over a table that is resident anyway.
//! Publishing it would give it a file, and a file's bytes are
//! [`crate::codec`]'s.

use crate::core::geometry::{CellId, CellMap};
use crate::core::moments::Moments;
use crate::records::PopulatedSystem;
use crate::tree::index::Index;
use elite_journal::prelude::{
    Allegiance, Economy, Government, Power, PowerplayState, Security, State,
};

/// A reading a histogram counts in buckets: one a variant, plus bucket zero
/// for a system nothing has reported it of — security excepted, where nothing
/// on record is anarchy; see its impl.
///
/// Zero is not the same fact as the variant `None`, which is the game saying
/// a populated system has none: both draw grey and the two are kept apart
/// anyway, for the same reason an absent `factions.bin` is not an empty one.
///
/// A trait because the readings are `elite_journal`'s types, which take no
/// inherent methods here. Each impl is a `match` and never a comparison:
/// `Allegiance`, `Economy` and `State` carry a hand-written `PartialEq` under
/// which `None != None`, so `==` answers falsely for the one variant a
/// histogram most needs to place.
pub trait Bucketed: Sized {
    /// How many buckets, the unknown at zero included.
    const BUCKETS: usize;

    /// Which bucket a system's reading counts in.
    fn bucket(of: Option<Self>) -> usize;

    /// The reading a bucket counts: the inverse of [`bucket`](Self::bucket),
    /// so a view can name the colour a bucket is drawn in.
    fn at(bucket: usize) -> Option<Self>;
}

impl Bucketed for Allegiance {
    const BUCKETS: usize = 11;

    fn bucket(of: Option<Allegiance>) -> usize {
        match of {
            None => 0,
            Some(Allegiance::Alliance) => 1,
            Some(Allegiance::Empire) => 2,
            Some(Allegiance::Federation) => 3,
            Some(Allegiance::Guardian) => 4,
            Some(Allegiance::Independent) => 5,
            Some(Allegiance::PilotsFederation) => 6,
            Some(Allegiance::PlayerPilots) => 7,
            Some(Allegiance::Thargoid) => 8,
            Some(Allegiance::FrontlineSolutions) => 9,
            Some(Allegiance::None) => 10,
        }
    }

    fn at(bucket: usize) -> Option<Allegiance> {
        match bucket {
            1 => Some(Allegiance::Alliance),
            2 => Some(Allegiance::Empire),
            3 => Some(Allegiance::Federation),
            4 => Some(Allegiance::Guardian),
            5 => Some(Allegiance::Independent),
            6 => Some(Allegiance::PilotsFederation),
            7 => Some(Allegiance::PlayerPilots),
            8 => Some(Allegiance::Thargoid),
            9 => Some(Allegiance::FrontlineSolutions),
            10 => Some(Allegiance::None),
            _ => None,
        }
    }
}

impl Bucketed for Government {
    const BUCKETS: usize = 18;

    fn bucket(of: Option<Government>) -> usize {
        match of {
            None => 0,
            Some(Government::Anarchy) => 1,
            Some(Government::Communism) => 2,
            Some(Government::Confederacy) => 3,
            Some(Government::Cooperative) => 4,
            Some(Government::Corporate) => 5,
            Some(Government::Democracy) => 6,
            Some(Government::Dictatorship) => 7,
            Some(Government::Feudal) => 8,
            Some(Government::Patronage) => 9,
            Some(Government::Prison) => 10,
            Some(Government::PrisonColony) => 11,
            Some(Government::Theocracy) => 12,
            Some(Government::Engineer) => 13,
            Some(Government::Carrier) => 14,
            Some(Government::Megaconstruction) => 15,
            Some(Government::PrivateOwnership) => 16,
            Some(Government::None) => 17,
        }
    }

    fn at(bucket: usize) -> Option<Government> {
        match bucket {
            1 => Some(Government::Anarchy),
            2 => Some(Government::Communism),
            3 => Some(Government::Confederacy),
            4 => Some(Government::Cooperative),
            5 => Some(Government::Corporate),
            6 => Some(Government::Democracy),
            7 => Some(Government::Dictatorship),
            8 => Some(Government::Feudal),
            9 => Some(Government::Patronage),
            10 => Some(Government::Prison),
            11 => Some(Government::PrisonColony),
            12 => Some(Government::Theocracy),
            13 => Some(Government::Engineer),
            14 => Some(Government::Carrier),
            15 => Some(Government::Megaconstruction),
            16 => Some(Government::PrivateOwnership),
            17 => Some(Government::None),
            _ => None,
        }
    }
}

/// Security is the one axis without an unreported bucket of its own. Anarchy
/// is the absence of security, and a store keeps it as no reading at all (see
/// [`elite_journal`]'s `Nullable`), so nothing on record and an anarchy are one
/// fact: bucket zero, which is [`Security::Anarchy`].
impl Bucketed for Security {
    const BUCKETS: usize = 4;

    fn bucket(of: Option<Security>) -> usize {
        match of {
            None | Some(Security::Anarchy) => 0,
            Some(Security::High) => 1,
            Some(Security::Medium) => 2,
            Some(Security::Low) => 3,
        }
    }

    fn at(bucket: usize) -> Option<Security> {
        match bucket {
            0 => Some(Security::Anarchy),
            1 => Some(Security::High),
            2 => Some(Security::Medium),
            3 => Some(Security::Low),
            _ => None,
        }
    }
}

impl Bucketed for Economy {
    const BUCKETS: usize = 18;

    fn bucket(of: Option<Economy>) -> usize {
        match of {
            None => 0,
            Some(Economy::Agriculture) => 1,
            Some(Economy::Colony) => 2,
            Some(Economy::Extraction) => 3,
            Some(Economy::HighTech) => 4,
            Some(Economy::Industrial) => 5,
            Some(Economy::Military) => 6,
            Some(Economy::Refinery) => 7,
            Some(Economy::Service) => 8,
            Some(Economy::Terraforming) => 9,
            Some(Economy::Tourism) => 10,
            Some(Economy::Carrier) => 11,
            Some(Economy::Prison) => 12,
            Some(Economy::Rescue) => 13,
            Some(Economy::PrivateEnterprise) => 14,
            Some(Economy::Repair) => 15,
            Some(Economy::Undefined) => 16,
            Some(Economy::None) => 17,
        }
    }

    fn at(bucket: usize) -> Option<Economy> {
        match bucket {
            1 => Some(Economy::Agriculture),
            2 => Some(Economy::Colony),
            3 => Some(Economy::Extraction),
            4 => Some(Economy::HighTech),
            5 => Some(Economy::Industrial),
            6 => Some(Economy::Military),
            7 => Some(Economy::Refinery),
            8 => Some(Economy::Service),
            9 => Some(Economy::Terraforming),
            10 => Some(Economy::Tourism),
            11 => Some(Economy::Carrier),
            12 => Some(Economy::Prison),
            13 => Some(Economy::Rescue),
            14 => Some(Economy::PrivateEnterprise),
            15 => Some(Economy::Repair),
            16 => Some(Economy::Undefined),
            17 => Some(Economy::None),
            _ => None,
        }
    }
}

/// The controlling faction's state. [`State::None`] is the game saying the
/// faction is in none, which is the commonest reading and not the same fact
/// as nothing on record.
impl Bucketed for State {
    const BUCKETS: usize = 28;

    fn bucket(of: Option<State>) -> usize {
        match of {
            None => 0,
            Some(State::Blight) => 1,
            Some(State::Boom) => 2,
            Some(State::Bust) => 3,
            Some(State::CivilLiberty) => 4,
            Some(State::CivilUnrest) => 5,
            Some(State::CivilWar) => 6,
            Some(State::ColdWar) => 7,
            Some(State::Colonisation) => 8,
            Some(State::Drought) => 9,
            Some(State::Election) => 10,
            Some(State::Expansion) => 11,
            Some(State::Famine) => 12,
            Some(State::HistoricEvent) => 13,
            Some(State::InfrastructureFailure) => 14,
            Some(State::Investment) => 15,
            Some(State::Lockdown) => 16,
            Some(State::NaturalDisaster) => 17,
            Some(State::Outbreak) => 18,
            Some(State::PirateAttack) => 19,
            Some(State::PublicHoliday) => 20,
            Some(State::Retreat) => 21,
            Some(State::Revolution) => 22,
            Some(State::TechnologicalLeap) => 23,
            Some(State::Terrorism) => 24,
            Some(State::TradeWar) => 25,
            Some(State::War) => 26,
            Some(State::None) => 27,
        }
    }

    fn at(bucket: usize) -> Option<State> {
        match bucket {
            1 => Some(State::Blight),
            2 => Some(State::Boom),
            3 => Some(State::Bust),
            4 => Some(State::CivilLiberty),
            5 => Some(State::CivilUnrest),
            6 => Some(State::CivilWar),
            7 => Some(State::ColdWar),
            8 => Some(State::Colonisation),
            9 => Some(State::Drought),
            10 => Some(State::Election),
            11 => Some(State::Expansion),
            12 => Some(State::Famine),
            13 => Some(State::HistoricEvent),
            14 => Some(State::InfrastructureFailure),
            15 => Some(State::Investment),
            16 => Some(State::Lockdown),
            17 => Some(State::NaturalDisaster),
            18 => Some(State::Outbreak),
            19 => Some(State::PirateAttack),
            20 => Some(State::PublicHoliday),
            21 => Some(State::Retreat),
            22 => Some(State::Revolution),
            23 => Some(State::TechnologicalLeap),
            24 => Some(State::Terrorism),
            25 => Some(State::TradeWar),
            26 => Some(State::War),
            27 => Some(State::None),
            _ => None,
        }
    }
}

/// The controlling power. No variant says a system has none, so bucket zero
/// is both nothing on record and no power holding it.
impl Bucketed for Power {
    const BUCKETS: usize = 14;

    fn bucket(of: Option<Power>) -> usize {
        match of {
            None => 0,
            Some(Power::AislingDuval) => 1,
            Some(Power::ArchonDelaine) => 2,
            Some(Power::ArissaLavignyDuval) => 3,
            Some(Power::DentonPatreus) => 4,
            Some(Power::EdmundMahon) => 5,
            Some(Power::FeliciaWinters) => 6,
            Some(Power::JeromeArcher) => 7,
            Some(Power::LiYongRui) => 8,
            Some(Power::NakatoKaine) => 9,
            Some(Power::PranavAntal) => 10,
            Some(Power::YuriGrom) => 11,
            Some(Power::ZacharyHudson) => 12,
            Some(Power::ZeminaTorval) => 13,
        }
    }

    fn at(bucket: usize) -> Option<Power> {
        match bucket {
            1 => Some(Power::AislingDuval),
            2 => Some(Power::ArchonDelaine),
            3 => Some(Power::ArissaLavignyDuval),
            4 => Some(Power::DentonPatreus),
            5 => Some(Power::EdmundMahon),
            6 => Some(Power::FeliciaWinters),
            7 => Some(Power::JeromeArcher),
            8 => Some(Power::LiYongRui),
            9 => Some(Power::NakatoKaine),
            10 => Some(Power::PranavAntal),
            11 => Some(Power::YuriGrom),
            12 => Some(Power::ZacharyHudson),
            13 => Some(Power::ZeminaTorval),
            _ => None,
        }
    }
}

/// Where a system stands in Powerplay. Like [`Power`], bucket zero is both
/// nothing on record and outside every power's reach.
impl Bucketed for PowerplayState {
    const BUCKETS: usize = 11;

    fn bucket(of: Option<PowerplayState>) -> usize {
        match of {
            None => 0,
            Some(PowerplayState::InPrepareRadius) => 1,
            Some(PowerplayState::Prepared) => 2,
            Some(PowerplayState::Exploited) => 3,
            Some(PowerplayState::Contested) => 4,
            Some(PowerplayState::Controlled) => 5,
            Some(PowerplayState::Turmoil) => 6,
            Some(PowerplayState::HomeSystem) => 7,
            Some(PowerplayState::Unoccupied) => 8,
            Some(PowerplayState::Fortified) => 9,
            Some(PowerplayState::Stronghold) => 10,
        }
    }

    fn at(bucket: usize) -> Option<PowerplayState> {
        match bucket {
            1 => Some(PowerplayState::InPrepareRadius),
            2 => Some(PowerplayState::Prepared),
            3 => Some(PowerplayState::Exploited),
            4 => Some(PowerplayState::Contested),
            5 => Some(PowerplayState::Controlled),
            6 => Some(PowerplayState::Turmoil),
            7 => Some(PowerplayState::HomeSystem),
            8 => Some(PowerplayState::Unoccupied),
            9 => Some(PowerplayState::Fortified),
            10 => Some(PowerplayState::Stronghold),
            _ => None,
        }
    }
}

/// Every reading one inhabited system is counted by, as the populated table
/// holds them
///
/// [`Default`] is nothing on record along any of them.
#[derive(Copy, Clone, Debug, Default)]
pub struct Readings {
    pub allegiance: Option<Allegiance>,
    pub government: Option<Government>,
    pub security: Option<Security>,
    /// The primary economy; the secondary is what a system trades in
    /// besides, and is not what it is colored by.
    pub economy: Option<Economy>,
    pub state: Option<State>,
    pub power: Option<Power>,
    pub powerplay_state: Option<PowerplayState>,
}

impl Readings {
    /// The readings off a populated table's row
    pub fn of(row: &PopulatedSystem) -> Readings {
        Readings {
            allegiance: row.allegiance,
            government: row.government,
            security: row.security,
            economy: row.primary_economy,
            state: row.state,
            power: row.power,
            powerplay_state: row.powerplay_state,
        }
    }
}

/// Where each axis's buckets start in [`Inhabited`]'s one histogram, in the
/// order [`Readings`] lists them.
const ALLEGIANCE: usize = 0;
const GOVERNMENT: usize = ALLEGIANCE + Allegiance::BUCKETS;
const SECURITY: usize = GOVERNMENT + Government::BUCKETS;
const ECONOMY: usize = SECURITY + Security::BUCKETS;
const STATE: usize = ECONOMY + Economy::BUCKETS;
const POWER: usize = STATE + State::BUCKETS;
const POWERPLAY_STATE: usize = POWER + Power::BUCKETS;
/// Every axis's buckets, end to end.
const BUCKETS: usize = POWERPLAY_STATE + PowerplayState::BUCKETS;

/// What a cell carries about the inhabited systems in its whole subtree.
///
/// Built from single systems with [`of_system`](Self::of_system), rolled up
/// with [`merge`](Self::merge) and drawn over its own loaded slice through
/// [`remove`](Self::remove). Every field is a sum, so a set split any way and
/// rejoined is the same record and there is no non-additive key to answer on
/// the total instead of the residual — which
/// [`Aggregate`](crate::core::aggregate::Aggregate) needs for `m_min` and this
/// does not need at all.
///
/// The prune key is `count > 0`: a subtree with nobody in it cannot matter to
/// a political view, and that is free to ask.
///
/// [`Default`] is [`ZERO`](Self::ZERO): a record of nobody is the identity of
/// [`merge`](Self::merge), so the two cannot mean different things.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Inhabited {
    /// How many systems in the subtree anybody lives in.
    count: u64,
    /// Position moments in that weight, one unit a system: the centroid the
    /// political field splats from and the spread of its footprint.
    settled: Moments,
    /// Inhabited systems per bucket of every axis, the axes end to end at the
    /// offsets above. Each axis's run sums to `count`.
    ///
    /// One array rather than one an axis, so a merge and a removal are one
    /// loop each whatever the axes are.
    buckets: [u32; BUCKETS],
}

impl Default for Inhabited {
    fn default() -> Inhabited {
        Inhabited::ZERO
    }
}

impl Inhabited {
    /// The empty record, the identity of [`merge`](Self::merge).
    pub const ZERO: Inhabited =
        Inhabited { count: 0, settled: Moments::ZERO, buckets: [0; BUCKETS] };

    /// One inhabited system's contribution: one unit of weight at its
    /// position, and one count in each axis's bucket.
    ///
    /// The caller owes the predicate. Only a system somebody lives in belongs
    /// here — `population > 0` — because the weight is what makes the centroid
    /// the colonies' own, and an empty system contributing would pull it back
    /// toward the count centroid this exists to differ from.
    pub fn of_system(position: [f64; 3], readings: Readings) -> Inhabited {
        let mut buckets = [0; BUCKETS];
        buckets[ALLEGIANCE + Allegiance::bucket(readings.allegiance)] = 1;
        buckets[GOVERNMENT + Government::bucket(readings.government)] = 1;
        buckets[SECURITY + Security::bucket(readings.security)] = 1;
        buckets[ECONOMY + Economy::bucket(readings.economy)] = 1;
        buckets[STATE + State::bucket(readings.state)] = 1;
        buckets[POWER + Power::bucket(readings.power)] = 1;
        buckets[POWERPLAY_STATE
            + PowerplayState::bucket(readings.powerplay_state)] = 1;
        Inhabited { count: 1, settled: Moments::point(1.0, position), buckets }
    }

    /// Roll two records into one. Commutative and associative, so a subtree
    /// rolls up the same however its children are ordered.
    pub fn merge(self, other: Inhabited) -> Inhabited {
        let mut buckets = self.buckets;
        for (a, o) in buckets.iter_mut().zip(other.buckets) {
            *a += o;
        }
        Inhabited {
            count: self.count + other.count,
            settled: self.settled.merge(other.settled),
            buckets,
        }
    }

    /// The residual of this total less a slice that was part of it: what a
    /// cell splats once some of its systems have loaded and are drawn as
    /// themselves.
    ///
    /// Every field subtracts exactly, being the inverse of
    /// [`merge`](Self::merge), the moments included.
    pub fn remove(self, slice: Inhabited) -> Inhabited {
        let mut buckets = self.buckets;
        for (a, s) in buckets.iter_mut().zip(slice.buckets) {
            *a -= s;
        }
        Inhabited {
            count: self.count - slice.count,
            settled: self.settled.remove(slice.settled),
            buckets,
        }
    }

    /// How many systems in the subtree anybody lives in, exact. Zero is the
    /// prune key: nothing political is under this cell at all.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Where the political field splats from: the centre of the inhabited
    /// systems alone, or [`None`] where there are none.
    ///
    /// [`None`] is load-bearing and must not fall back on the count centroid.
    /// A cell with nobody in it has no political place, and drawing one at the
    /// centre of its empty systems is how a colony appears where there is not
    /// one.
    pub fn centroid(&self) -> Option<[f64; 3]> {
        self.settled.centroid()
    }

    /// The footprint of that field: the RMS radius of the inhabited systems
    /// about their own centroid, in light years.
    pub fn spread(&self) -> f64 {
        self.settled.rms_radius()
    }

    /// One axis's run of the histogram, `B::BUCKETS` long from `at`
    fn run<B: Bucketed>(&self, at: usize) -> &[u32] {
        &self.buckets[at..at + B::BUCKETS]
    }

    /// Inhabited systems per allegiance bucket, which a political view resolves
    /// its colour from. Index with [`Allegiance::bucket`](Bucketed::bucket), name with
    /// [`Allegiance::at`](Bucketed::at). Every axis below is read the same way.
    pub fn allegiance(&self) -> &[u32] {
        self.run::<Allegiance>(ALLEGIANCE)
    }

    /// Inhabited systems per government bucket.
    pub fn government(&self) -> &[u32] {
        self.run::<Government>(GOVERNMENT)
    }

    /// Inhabited systems per security bucket.
    pub fn security(&self) -> &[u32] {
        self.run::<Security>(SECURITY)
    }

    /// Inhabited systems per primary economy bucket.
    pub fn economy(&self) -> &[u32] {
        self.run::<Economy>(ECONOMY)
    }

    /// Inhabited systems per controlling faction state bucket.
    pub fn state(&self) -> &[u32] {
        self.run::<State>(STATE)
    }

    /// Inhabited systems per controlling power bucket.
    pub fn power(&self) -> &[u32] {
        self.run::<Power>(POWER)
    }

    /// Inhabited systems per Powerplay state bucket.
    pub fn powerplay_state(&self) -> &[u32] {
        self.run::<PowerplayState>(POWERPLAY_STATE)
    }
}

impl FromIterator<Inhabited> for Inhabited {
    fn from_iter<I: IntoIterator<Item = Inhabited>>(iter: I) -> Inhabited {
        iter.into_iter().fold(Inhabited::ZERO, Inhabited::merge)
    }
}

/// The inhabited aggregation over a whole tree, one record a cell.
///
/// Keyed by address and never by ordinal, which is the same rule the published
/// per-column files are held to: `index.bin` has no stable cell order, so
/// anything keyed by file position is rewritten whole whenever the tree's shape
/// moves.
///
/// Sparse on purpose. Only the cells with somebody under them get a record —
/// tens of thousands of a few hundred thousand — and a cell absent from here
/// reads as [`Inhabited::ZERO`], which is what it is.
#[derive(Clone, Debug, Default)]
pub struct Inhabitance(CellMap<Inhabited>);

impl Inhabitance {
    /// Roll every inhabited system in `rows` up the tree it falls in.
    ///
    /// A system contributes to every cell on its path from the root, which is
    /// what makes a cell's record the total over its whole subtree and what
    /// makes a colony's colour reach every level above it. Cells the tree does
    /// not hold are not invented: the descent follows the index's own children,
    /// so a row lands on exactly the cells that stand over it.
    ///
    /// Rows with nobody living in them are skipped. `populated.bin` is written
    /// from a projection already gated on population, so this is a guard and
    /// not a filter — but the weight is the whole point of the record, and a
    /// silent zero-population row would flatten the centroid it exists to
    /// sharpen.
    ///
    /// Positions come off the populated row as `f32`, which is a thirtieth of
    /// a light year out at the galaxy's edge and can put a system the wrong
    /// side of a cell boundary it sits exactly on. That moves one count between
    /// two neighbouring cells of a density field and is not worth a wider
    /// column to prevent.
    pub fn of<'a>(
        index: &Index,
        rows: impl IntoIterator<Item = &'a PopulatedSystem>,
    ) -> Inhabitance {
        let mut held: CellMap<Inhabited> = CellMap::default();
        for row in rows {
            if row.population == 0 {
                continue;
            }
            let position = [
                row.position[0] as f64,
                row.position[1] as f64,
                row.position[2] as f64,
            ];
            let one = Inhabited::of_system(position, Readings::of(row));
            index.descend(position, |id| {
                let at = held.entry(id).or_insert(Inhabited::ZERO);
                *at = at.merge(one);
            });
        }
        Inhabitance(held)
    }

    /// What a cell carries, or [`None`] where nobody lives under it.
    pub fn get(&self, id: CellId) -> Option<&Inhabited> {
        self.0.get(&id)
    }

    /// How many cells have anybody under them.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no cell does.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::aggregate::Aggregate;
    use crate::core::name::SystemName;
    use crate::tree::cell::Cell;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn close3(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(a, b)| close(*a, *b))
    }

    /// An inhabited system at a place, with an allegiance and nothing else.
    fn row(
        address: i64,
        at: [f64; 3],
        population: u64,
        allegiance: Option<Allegiance>,
    ) -> PopulatedSystem {
        PopulatedSystem {
            address,
            name: SystemName::new("SOL"),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
            population,
            security: None,
            government: None,
            allegiance,
            primary_economy: None,
            secondary_economy: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
            state: None,
            power: None,
            powerplay_state: None,
        }
    }

    fn of(at: [f64; 3], allegiance: Option<Allegiance>) -> Inhabited {
        Inhabited::of_system(at, Readings { allegiance, ..Readings::default() })
    }

    #[test]
    fn a_system_is_its_own_record() {
        let a = of([1.0, 2.0, 3.0], Some(Allegiance::Empire));
        assert_eq!(a.count(), 1);
        assert!(close3(a.centroid().unwrap(), [1.0, 2.0, 3.0]));
        assert!(close(a.spread(), 0.0));
        assert_eq!(
            a.allegiance()[Allegiance::bucket(Some(Allegiance::Empire))],
            1
        );
        for run in [
            a.allegiance(),
            a.government(),
            a.security(),
            a.economy(),
            a.state(),
            a.power(),
            a.powerplay_state(),
        ] {
            assert_eq!(run.iter().sum::<u32>(), 1);
        }
    }

    /// Every reading on a row counts in its own axis's bucket, so no axis's
    /// run of the one histogram reads another's.
    #[test]
    fn every_reading_lands_on_its_own_axis() {
        let a = Inhabited::of_system(
            [0.0; 3],
            Readings::of(&PopulatedSystem {
                primary_economy: Some(Economy::HighTech),
                state: Some(State::Boom),
                power: Some(Power::ZeminaTorval),
                powerplay_state: Some(PowerplayState::Stronghold),
                ..row(1, [0.0; 3], 1, Some(Allegiance::Empire))
            }),
        );
        assert_eq!(
            a.allegiance()[Allegiance::bucket(Some(Allegiance::Empire))],
            1
        );
        assert_eq!(a.government()[Government::bucket(None)], 1);
        assert_eq!(a.economy()[Economy::bucket(Some(Economy::HighTech))], 1);
        assert_eq!(a.state()[State::bucket(Some(State::Boom))], 1);
        assert_eq!(a.power()[Power::bucket(Some(Power::ZeminaTorval))], 1);
        assert_eq!(
            a.powerplay_state()
                [PowerplayState::bucket(Some(PowerplayState::Stronghold))],
            1
        );
    }

    /// Every bucket names exactly the value that counts in it, and the widths
    /// are tight: no variant shares a bucket and no bucket goes unused.
    #[test]
    fn the_buckets_round_trip() {
        fn round_trips<T: Bucketed>() {
            for bucket in 0..T::BUCKETS {
                assert_eq!(T::bucket(T::at(bucket)), bucket);
            }
        }
        round_trips::<Allegiance>();
        round_trips::<Government>();
        round_trips::<Security>();
        round_trips::<Economy>();
        round_trips::<State>();
        round_trips::<Power>();
        round_trips::<PowerplayState>();
    }

    /// The game saying "no allegiance" is not the same fact as nothing having
    /// been reported, and the histogram keeps them apart. Neither can be found
    /// by comparison: `Allegiance::None != Allegiance::None`.
    #[test]
    fn unreported_and_unaligned_are_different_buckets() {
        assert_ne!(
            Allegiance::bucket(None),
            Allegiance::bucket(Some(Allegiance::None))
        );
        let unreported = of([0.0; 3], None);
        let unaligned = of([0.0; 3], Some(Allegiance::None));
        assert_ne!(unreported.allegiance(), unaligned.allegiance());
    }

    /// A set split any way and rejoined is the same record, moments included.
    #[test]
    fn a_split_conserves_the_subtree() {
        let systems = [
            ([0.0, 0.0, 0.0], Some(Allegiance::Federation)),
            ([10.0, 0.0, 0.0], Some(Allegiance::Empire)),
            ([0.0, 10.0, 0.0], Some(Allegiance::Federation)),
            ([-5.0, 2.0, 8.0], None),
        ];
        let whole: Inhabited = systems.iter().map(|&(p, a)| of(p, a)).collect();
        let left: Inhabited =
            systems[..2].iter().map(|&(p, a)| of(p, a)).collect();
        let right: Inhabited =
            systems[2..].iter().map(|&(p, a)| of(p, a)).collect();

        assert_eq!(whole.count(), left.merge(right).count());
        assert_eq!(whole.allegiance(), left.merge(right).allegiance());
        assert!(close3(
            whole.centroid().unwrap(),
            left.merge(right).centroid().unwrap()
        ));
        assert!(close(whole.spread(), left.merge(right).spread()));
        // And the other order, since a rollup does not fix one.
        assert!(close(whole.spread(), right.merge(left).spread()));
    }

    /// `remove` is the exact inverse of `merge`: the residual of a total less
    /// a loaded slice is the rest, so nothing is counted twice.
    #[test]
    fn remove_leaves_the_residual() {
        let slice: Inhabited = [
            ([1.0, 0.0, 0.0], Some(Allegiance::Alliance)),
            ([2.0, 1.0, 0.0], Some(Allegiance::Empire)),
        ]
        .iter()
        .map(|&(p, a)| of(p, a))
        .collect();
        let rest: Inhabited = [
            ([20.0, 5.0, 5.0], Some(Allegiance::Federation)),
            ([18.0, 4.0, 7.0], None),
            ([25.0, 9.0, 1.0], Some(Allegiance::None)),
        ]
        .iter()
        .map(|&(p, a)| of(p, a))
        .collect();
        let residual = slice.merge(rest).remove(slice);

        assert_eq!(residual.count(), rest.count());
        assert_eq!(residual.allegiance(), rest.allegiance());
        assert_eq!(residual.government(), rest.government());
        assert_eq!(residual.security(), rest.security());
        assert_eq!(residual.state(), rest.state());
        assert!(close3(residual.centroid().unwrap(), rest.centroid().unwrap()));
        assert!(close(residual.spread(), rest.spread()));
    }

    #[test]
    fn zero_is_the_identity() {
        let a = of([1.0, 2.0, 3.0], Some(Allegiance::Empire));
        assert_eq!(a.merge(Inhabited::ZERO), a);
        assert_eq!(Inhabited::ZERO.merge(a), a);
        assert_eq!(Inhabited::ZERO.count(), 0);
    }

    /// An empty record has no political place, and says so rather than
    /// answering with the origin.
    #[test]
    fn nobody_home_has_no_centroid() {
        assert_eq!(Inhabited::ZERO.centroid(), None);
        assert!(close(Inhabited::ZERO.spread(), 0.0));
    }

    /// A tree of the chain of cells from the root down to level 3 over `at`,
    /// each holding the next as its one child, so a descent has somewhere to
    /// go.
    fn tree(at: [f64; 3]) -> Index {
        let leaf = CellId::of_point(at, 3);
        let mut cells = vec![Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 0,
            child_mask: 0,
            aggregate: Aggregate::ZERO,
        }];
        // The chain of cells over `at`, each naming the next as its child.
        let mut path = vec![CellId::ROOT];
        for level in 1..=leaf.level {
            path.push(CellId::of_point(at, level));
        }
        cells.clear();
        for (depth, id) in path.iter().enumerate() {
            let mask = match path.get(depth + 1) {
                Some(kid) => {
                    let kids = id.children();
                    let octant =
                        kids.iter().position(|k| k == kid).unwrap() as u8;
                    1u8 << octant
                }
                None => 0,
            };
            cells.push(Cell {
                id: *id,
                rank_lo: 0,
                rank_hi: 0,
                child_mask: mask,
                aggregate: Aggregate::ZERO,
            });
        }
        Index::from_cells(cells)
    }

    /// Every cell over a system counts it, so a colony's colour reaches the
    /// root and a coarse view is the sum of the fine ones under it.
    #[test]
    fn a_system_counts_in_every_cell_over_it() {
        let at = [100.0, 20.0, 24_000.0];
        let index = tree(at);
        let held = Inhabitance::of(
            &index,
            [&row(1, at, 5_000, Some(Allegiance::Empire))],
        );

        let root = held.get(CellId::ROOT).expect("the root counts it");
        assert_eq!(root.count(), 1);
        assert!(close3(root.centroid().unwrap(), at));
        for level in 1..=3u8 {
            let id = CellId::of_point(at, level);
            assert_eq!(
                held.get(id).map(Inhabited::count),
                Some(1),
                "level {level} lost the system"
            );
        }
    }

    /// Nobody living there is not a colony, whatever the table says.
    #[test]
    fn an_empty_system_is_not_counted() {
        let at = [100.0, 20.0, 24_000.0];
        let index = tree(at);
        let held = Inhabitance::of(&index, [&row(1, at, 0, None)]);
        assert!(held.is_empty());
        assert_eq!(held.get(CellId::ROOT), None);
    }

    /// The point of the record: the inhabited centre is not the count centre.
    ///
    /// Two colonies at one end of a cell and a crowd of empty systems at the
    /// other. The count centroid sits with the crowd and the political field
    /// laid there would draw the colonies where they are not; this centroid
    /// sits on the colonies.
    #[test]
    fn the_settled_centre_is_not_the_count_centre() {
        let colonies = [[10.0, 0.0, 24_000.0], [12.0, 0.0, 24_000.0]];
        let index = tree(colonies[0]);
        let held = Inhabitance::of(
            &index,
            [
                &row(1, colonies[0], 100, Some(Allegiance::Empire)),
                &row(2, colonies[1], 100, Some(Allegiance::Empire)),
            ],
        );
        let settled = held.get(CellId::ROOT).unwrap().centroid().unwrap();

        // The stellar count centroid of the same region, pulled far off by a
        // hundred empty systems nowhere near the colonies.
        let mut mass = Aggregate::ZERO;
        for i in 0..100 {
            mass = mass.merge(Aggregate::of_system(
                [-20_000.0 + i as f64, 0.0, 24_000.0],
                0,
                crate::core::star::StarKind::Unknown,
            ));
        }
        for at in colonies {
            mass = mass.merge(Aggregate::of_system(
                at,
                0,
                crate::core::star::StarKind::Unknown,
            ));
        }
        let count = mass.count_centroid().unwrap();

        assert!(close(settled[0], 11.0), "settled centre moved: {settled:?}");
        assert!(
            (count[0] - settled[0]).abs() > 1_000.0,
            "the two centres did not diverge: {count:?} vs {settled:?}"
        );
    }
}
