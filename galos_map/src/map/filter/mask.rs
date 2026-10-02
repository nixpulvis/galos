//! What the user has asked not to see, by the color a system is drawn in
//!
//! Every color of every axis is a toggle, and a toggled one hides the systems
//! drawn in it. The mask is kept on the index's own buckets ([`Bucketed`])
//! rather than on [`Hue`]: seven governments share red, and each has to be
//! hidden on its own.
//!
//! **Every axis hiding something applies, and each says so in a row of its
//! own.** Hide High along security, color by state and hide Expansion, and
//! the map is the systems that are neither: the color row under the bar
//! stands for state, and security keeps a row beside it saying what it
//! hides, chips and close and all. A system is never missing for a color
//! that is not on screen with nothing on screen to say why; the row is what
//! says it. See `ui::bar::applied::color_row`.
//!
//! So two values of one axis are either (High or Medium security), and two
//! axes are both (that security, and that state). Nothing more is asked of
//! the user to say which: an axis is a row, and every row narrows.
//!
//! The systems nobody lives in are the one flag no axis owns, having no
//! reading on any of them. It belongs to the color row, whose chip it is,
//! and applies along whichever political axis is drawn. A security row
//! hiding High hides no empty system, an empty system not being High.
//!
//! It narrows and never adds. The picking filters say what the user asked to
//! see, and the mask cuts into that, as a span does; see
//! [`crate::map::filter::Filters::admits`].
//!
//! A new axis is a run of [`Inhabited`]'s histogram, a variant of
//! [`ColorBy`] and an arm in each `match` below, a [`Hue`] mapping and a
//! layout in [`super::key`]: the mask, the key and the field all walk
//! [`ColorBy::ALL`], so none of them has a list of its own to keep up.

use crate::map::galaxy::spawn::{ColorBy, Hue};
use elite_journal::prelude::{
    Allegiance, Economy, Government, Power, PowerplayState, Security, State,
};
use galos_index::prelude::StarKind;
use galos_index::read::inhabited::{Bucketed, Inhabited, Readings};
use galos_index::records::PopulatedSystem;

/// A colony's readings, by the bucket each counts in along every political
/// axis
///
/// What the mask asks about a system, in the index's own terms, so a system
/// on the map, a payload point joined against the populated table and a cell's
/// histogram are all read by the one numbering. [`None`] where a candidate
/// carries one of these is a system nobody lives in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Buckets([u8; ColorBy::POLITICAL.len()]);

impl Buckets {
    /// The buckets a system's readings count in
    pub(crate) fn of(readings: &Readings) -> Buckets {
        // No axis has more than 32 buckets, which a byte holds.
        Buckets(ColorBy::POLITICAL.map(|axis| axis.bucket(readings) as u8))
    }

    /// The buckets a populated table's row counts in, where anybody lives there
    ///
    /// Inhabited is `population > 0`, exactly as [`Inhabited`] counts it, so a
    /// row the histograms leave out is one the mask reads as uninhabited too
    /// and the field and the marks cannot disagree about which it is.
    pub(crate) fn of_row(row: &PopulatedSystem) -> Option<Buckets> {
        (row.population > 0).then(|| Buckets::of(&Readings::of(row)))
    }

    /// The bucket this system counts in along a political `axis`
    pub(crate) fn on(self, axis: ColorBy) -> usize {
        usize::from(self.0[axis.slot()])
    }
}

/// What the key counts each value of an axis in: the galaxy's colonies along
/// a political axis, and every system's star along star class
///
/// Owned, the root's being a copy a frame: the key is drawn from a
/// parameter it also writes the coloring through.
#[derive(Copy, Clone)]
pub(crate) struct Held {
    pub(crate) colonies: Inhabited,
    /// Systems by [`StarKind::code`], the root cell's
    pub(crate) stars: [u32; StarKind::COUNT],
}

/// Every axis's buckets fit a [`Mask`] word with a bit to spare, which
/// [`Mask::invert`]'s shift needs.
const _: () = {
    let mut at = 0;
    while at < ColorBy::ALL.len() {
        assert!(ColorBy::ALL[at].buckets() < 32);
        at += 1;
    }
};

impl ColorBy {
    /// Every axis the map can be colored by, in the order the key lists them
    pub const ALL: [ColorBy; 8] = [
        ColorBy::StarClass,
        ColorBy::Allegiance,
        ColorBy::Government,
        ColorBy::Security,
        ColorBy::Economy,
        ColorBy::State,
        ColorBy::Power,
        ColorBy::PowerplayState,
    ];

    /// The axes read off a colony's populated columns, which are the ones a
    /// [`Buckets`] holds
    pub const POLITICAL: [ColorBy; 7] = [
        ColorBy::Allegiance,
        ColorBy::Government,
        ColorBy::Security,
        ColorBy::Economy,
        ColorBy::State,
        ColorBy::Power,
        ColorBy::PowerplayState,
    ];

    /// Whether this axis colors every system rather than the colonies alone
    ///
    /// Star class, read off a system's payload point rather than its
    /// populated row: every system has a star, so there is no uninhabited
    /// gray for the axis to leave, and the uninhabited flag says nothing
    /// about it.
    pub const fn every_system(self) -> bool {
        matches!(self, ColorBy::StarClass)
    }

    /// Where this axis's bits are kept in a [`Mask`] and, for a political
    /// axis, its bucket in a [`Buckets`]: the political axes first, in
    /// [`Self::POLITICAL`]'s order. Not where the key lists it.
    const fn slot(self) -> usize {
        match self {
            ColorBy::Allegiance => 0,
            ColorBy::Government => 1,
            ColorBy::Security => 2,
            ColorBy::Economy => 3,
            ColorBy::State => 4,
            ColorBy::Power => 5,
            ColorBy::PowerplayState => 6,
            ColorBy::StarClass => 7,
        }
    }

    /// How many buckets this axis counts in, the unreported one included
    pub const fn buckets(self) -> usize {
        match self {
            ColorBy::Allegiance => Allegiance::BUCKETS,
            ColorBy::Government => Government::BUCKETS,
            ColorBy::Security => Security::BUCKETS,
            ColorBy::Economy => Economy::BUCKETS,
            ColorBy::State => State::BUCKETS,
            ColorBy::Power => Power::BUCKETS,
            ColorBy::PowerplayState => PowerplayState::BUCKETS,
            ColorBy::StarClass => StarKind::COUNT,
        }
    }

    /// The bucket a system reading `readings` counts in along this axis
    ///
    /// Zero along star class, whose bucket is no populated reading but the
    /// [`StarKind::code`] of the system's payload point: [`ColorBy::hue`]
    /// and [`Mask::admits`] read the kind itself, and nothing asks this.
    pub(crate) fn bucket(self, readings: &Readings) -> usize {
        match self {
            ColorBy::Allegiance => Allegiance::bucket(readings.allegiance),
            ColorBy::Government => Government::bucket(readings.government),
            ColorBy::Security => Security::bucket(readings.security),
            ColorBy::Economy => Economy::bucket(readings.economy),
            ColorBy::State => State::bucket(readings.state),
            ColorBy::Power => Power::bucket(readings.power),
            ColorBy::PowerplayState => {
                PowerplayState::bucket(readings.powerplay_state)
            }
            ColorBy::StarClass => 0,
        }
    }

    /// The color a bucket of this axis is drawn in
    ///
    /// The one mapping a mark, a cell and a key row are all painted by, so
    /// the key and the map cannot disagree about what a color means.
    pub(crate) fn hue_of(self, bucket: usize) -> Hue {
        match self {
            ColorBy::Allegiance => Hue::allegiance(Bucketed::at(bucket)),
            ColorBy::Government => Hue::government(Bucketed::at(bucket)),
            ColorBy::Security => Hue::security(Bucketed::at(bucket)),
            ColorBy::Economy => Hue::economy(Bucketed::at(bucket)),
            ColorBy::State => Hue::state(Bucketed::at(bucket)),
            ColorBy::Power => Hue::power(Bucketed::at(bucket)),
            ColorBy::PowerplayState => {
                Hue::powerplay_state(Bucketed::at(bucket))
            }
            ColorBy::StarClass => Hue::star(StarKind::from_code(bucket as u8)),
        }
    }

    /// The color a key swatch of `hue` is filled with along this axis
    ///
    /// Along star class at its class's
    /// [`star_level`](crate::map::paint::glow::star_level), under the
    /// brightest of them, so the key's swatches stand in the order the map's
    /// marks do. Every other axis draws its hues at one level, and fills
    /// them full.
    pub(crate) fn swatch(self, hue: Hue) -> bevy::color::Srgba {
        use crate::map::paint::glow::{STAR_LEVEL_TOP, star_level};
        match self {
            ColorBy::StarClass => hue.swatch(star_level(hue) / STAR_LEVEL_TOP),
            _ => hue.swatch(1.),
        }
    }

    /// A cell's colonies counted along this axis, bucket by bucket
    ///
    /// Nothing along star class: a colony's histogram carries no star, and
    /// the cell's stars are its aggregate's, [`Aggregate::kinds`].
    ///
    /// [`Aggregate::kinds`]: galos_index::core::aggregate::Aggregate::kinds
    pub(crate) fn counts(self, held: &Inhabited) -> &[u32] {
        match self {
            ColorBy::Allegiance => held.allegiance(),
            ColorBy::Government => held.government(),
            ColorBy::Security => held.security(),
            ColorBy::Economy => held.economy(),
            ColorBy::State => held.state(),
            ColorBy::Power => held.power(),
            ColorBy::PowerplayState => held.powerplay_state(),
            ColorBy::StarClass => &[],
        }
    }

    /// What the key counts along this axis, bucket by bucket
    pub(crate) fn counted(self, held: &Held) -> &[u32] {
        match self {
            ColorBy::StarClass => &held.stars,
            _ => self.counts(&held.colonies),
        }
    }

    /// What the axis is called, as the dropdown or a row says it
    pub fn name(self) -> &'static str {
        match self {
            ColorBy::Allegiance => "Allegiance",
            ColorBy::Government => "Government",
            ColorBy::Security => "Security",
            ColorBy::Economy => "Economy",
            ColorBy::State => "State",
            ColorBy::Power => "Power",
            ColorBy::PowerplayState => "Powerplay",
            ColorBy::StarClass => "Star class",
        }
    }
}

/// Which buckets of every axis are hidden, and which axis the map is colored
/// by
///
/// A bit a bucket, one word an axis: the largest axis is twenty-eight buckets.
/// Every word with a bit set is asked ([`Self::asks`]), the axis drawn and
/// the rest alike, each standing as a row under the bar.
///
/// Always applied. There is no lifting it as a filter's row is lifted: a
/// color shown is a chip clicked back on, and an axis shown whole is its
/// row's close.
///
/// Edited through [`crate::map::filter::Filters::edit_mask`] and nowhere else,
/// which is what counts an edit as a change to what the filters admit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    hidden: [u32; ColorBy::ALL.len()],
    /// The axis the map is colored by, and nothing in the realistic view,
    /// which colors the stars by their own light
    ///
    /// A copy of [`ColorBy`] and the view, kept here because every pass that
    /// asks the mask asks it about a system and not about the map, and
    /// followed from the resources by [`crate::map::filter::follow_color_by`].
    /// It says which axis a cell's colors are kept along ([`Self::keeps`])
    /// and the rest weighed by share ([`Self::off_axis`]), and whose row the
    /// uninhabited flag is a chip of. In the realistic view there is no such
    /// row, so the flag is kept for the map view; every axis hiding something
    /// still applies, each with its row.
    drawn: Option<ColorBy>,
    /// Whether the systems nobody lives in are hidden
    ///
    /// One flag shared by every axis: an uninhabited system has no reading on
    /// any of them, so there is nothing to tell apart.
    uninhabited: bool,
    /// Whether the map draws the systems nobody lives in at all
    ///
    /// Not while the sky is read as populations, which draws only colonies:
    /// there is nothing for the uninhabited flag to hide, so it is ignored
    /// and kept. Nor is star class asked then, the colonies drawn coming off
    /// the populated table with no star behind them; see [`Self::asks`].
    /// Followed from the settings by
    /// [`crate::map::filter::follow_color_by`].
    empty_drawn: bool,
}

impl Default for Mask {
    /// Hiding nothing.
    fn default() -> Mask {
        Mask {
            hidden: [0; ColorBy::ALL.len()],
            drawn: Some(ColorBy::StarClass),
            uninhabited: false,
            empty_drawn: true,
        }
    }
}

impl Mask {
    /// Whether `bucket` of `axis` is set to be hidden
    pub fn hides(&self, axis: ColorBy, bucket: usize) -> bool {
        self.hidden[axis.slot()] & (1 << bucket) != 0
    }

    /// Whether the systems nobody lives in are set to be hidden, drawn or
    /// not
    pub fn hides_uninhabited(&self) -> bool {
        self.uninhabited
    }

    /// Whether the map draws the systems nobody lives in, which is when
    /// hiding them means anything
    pub fn draws_uninhabited(&self) -> bool {
        self.empty_drawn
    }

    /// Draw the systems nobody lives in from here on, or not
    pub(crate) fn draw_uninhabited(&mut self, drawn: bool) {
        self.empty_drawn = drawn;
    }

    /// Whether the uninhabited flag is cutting anything off the map
    ///
    /// Only along a political axis drawn, the flag being a chip of that
    /// row: an axis that colors every system has no uninhabited systems to
    /// set apart, and the realistic view has no color row to carry it.
    pub(crate) fn hides_empty(&self) -> bool {
        self.uninhabited
            && self.empty_drawn
            && self.drawn.is_some_and(|drawn| !drawn.every_system())
    }

    /// Whether anything of `axis` is set to be hidden, asked or not
    ///
    /// What puts a row under the bar for it.
    pub fn hiding(&self, axis: ColorBy) -> bool {
        self.hidden[axis.slot()] != 0
    }

    /// Whether what `axis` hides is cutting anything off the map
    ///
    /// Anything hidden along it, drawn or not, except star class while the
    /// sky is read as populations: those colonies come off the populated
    /// table with no star behind them, so there is nothing to ask. Hidden
    /// and not asked is [`Self::suspended`], which its row says.
    pub(crate) fn asks(&self, axis: ColorBy) -> bool {
        self.hiding(axis) && (!axis.every_system() || self.empty_drawn)
    }

    /// Whether `axis` hides something that this view cannot ask about
    pub fn suspended(&self, axis: ColorBy) -> bool {
        self.hiding(axis) && !self.asks(axis)
    }

    /// Whether the mask is cutting anything off the map: anything hidden
    /// along any axis it can ask, or the systems nobody lives in where the
    /// color row carries them
    pub(crate) fn narrows(&self) -> bool {
        self.hides_empty() || ColorBy::ALL.iter().any(|axis| self.asks(*axis))
    }

    /// The axis the map is colored by, whose row carries the uninhabited
    /// flag; nothing in the realistic view
    pub fn drawn(&self) -> Option<ColorBy> {
        self.drawn
    }

    /// Ask along `axis` from here on, or not at all, keeping what every axis
    /// hides
    pub(crate) fn draw(&mut self, axis: Option<ColorBy>) {
        self.drawn = axis;
    }

    /// Whether a system reading `politics` and arriving at a `kind` of star
    /// is let through
    ///
    /// Every axis is asked, drawn or not, and a system has to pass them all.
    /// [`None`] is a system nobody lives in, which no political axis says
    /// anything about, only the uninhabited flag. Star class asks the kind
    /// of every system alike.
    pub(crate) fn admits(
        &self,
        politics: Option<Buckets>,
        kind: StarKind,
    ) -> bool {
        if self.asks(ColorBy::StarClass)
            && self.hides(ColorBy::StarClass, usize::from(kind.code()))
        {
            return false;
        }
        match politics {
            None => !self.hides_empty(),
            Some(buckets) => ColorBy::POLITICAL
                .iter()
                .all(|axis| !self.hides(*axis, buckets.on(*axis))),
        }
    }

    /// Which of a cell's buckets along `drawn` the mask lets through
    ///
    /// Exact, the mask being asked along the axis the histogram is read
    /// along. Everything where `drawn` is not the axis the map is colored
    /// by, which is a frame between the coloring changing and the mask
    /// following it. What the other axes hide is [`Self::off_axis`]'s.
    pub(crate) fn keeps(&self, drawn: ColorBy) -> Keeps {
        if Some(drawn) != self.drawn || !self.asks(drawn) {
            return Keeps::ALL;
        }
        Keeps { hidden: self.hidden[drawn.slot()] }
    }

    /// The share of the systems nobody lives in that is let through: all of
    /// them or none
    pub(crate) fn keeps_uninhabited(&self) -> f32 {
        match self.hides_empty() {
            true => 0.,
            false => 1.,
        }
    }

    /// What the axes other than the one drawn let through of a cell, by
    /// share
    ///
    /// **A cell counts each axis on its own, so it cannot say how many of
    /// its systems pass two at once.** A cell with half its colonies High
    /// security and half in Expansion may hold none that are both or every
    /// one. Each axis is taken as independent of the others, its share
    /// multiplied in, as the draw multiplies in the drawn axis's own
    /// [`Self::keeps`]. Every axis counted the same way is what keeps the
    /// picture from changing with the coloring: whichever axis is drawn,
    /// the shares multiplied are the same ones.
    ///
    /// `colonies` is the cell's histogram, for the political axes, and
    /// `stars` its aggregate's systems by [`StarKind::code`], for star
    /// class. A political axis says nothing of a system nobody lives in, so
    /// only star class reaches the backdrop.
    pub(crate) fn off_axis(
        &self,
        colonies: Option<&Inhabited>,
        stars: Option<&[u32; StarKind::COUNT]>,
    ) -> OffAxis {
        // Asked of every splat and merged mark a frame, and nothing hidden
        // anywhere is the ordinary case: nothing to work out.
        if self.hidden.iter().all(|bits| *bits == 0) {
            return OffAxis::ALL;
        }
        let kept = |axis: ColorBy, counts: &[u32]| {
            let (mut whole, mut kept) = (0u64, 0u64);
            for (bucket, count) in counts.iter().enumerate() {
                whole += u64::from(*count);
                if !self.hides(axis, bucket) {
                    kept += u64::from(*count);
                }
            }
            match whole {
                0 => 1.,
                whole => kept as f32 / whole as f32,
            }
        };
        let mut off = OffAxis::ALL;
        for axis in ColorBy::ALL {
            if Some(axis) == self.drawn || !self.asks(axis) {
                continue;
            }
            match axis.every_system() {
                true => {
                    if let Some(stars) = stars {
                        off.backdrop *= kept(axis, stars);
                    }
                }
                false => {
                    if let Some(colonies) = colonies {
                        let share = kept(axis, axis.counts(colonies));
                        off.colonies *= share;
                    }
                }
            }
        }
        off.colonies *= off.backdrop;
        off
    }

    /// Hide or show every bucket in `buckets` of `axis`
    pub fn set(
        &mut self,
        axis: ColorBy,
        buckets: impl IntoIterator<Item = usize>,
        hidden: bool,
    ) {
        let bits = &mut self.hidden[axis.slot()];
        for bucket in buckets {
            match hidden {
                true => *bits |= 1 << bucket,
                false => *bits &= !(1 << bucket),
            }
        }
    }

    /// Hide `buckets` of `axis` if any of them is shown, else show them all
    ///
    /// What a row standing for several buckets does, and a row of one is the
    /// same gesture: a group partly hidden is hidden the rest of the way first,
    /// that being what a click on something still partly there asks.
    pub fn toggle(&mut self, axis: ColorBy, buckets: &[usize]) {
        let shown = buckets.iter().any(|bucket| !self.hides(axis, *bucket));
        self.set(axis, buckets.iter().copied(), shown);
    }

    /// Show `buckets` of `axis` and nothing else of it, the systems nobody
    /// lives in included where the axis is the political one drawn
    ///
    /// The other axes are left as they were. Soloing an allegiance is asking
    /// to see only it, not to forget which governments were hidden; and
    /// soloing a star is not asking to hide empty space from the allegiances.
    /// Nor is soloing along another axis's row, which has no uninhabited chip
    /// on it to say it had.
    pub fn solo(&mut self, axis: ColorBy, buckets: &[usize]) {
        self.set(axis, 0..axis.buckets(), true);
        self.set(axis, buckets.iter().copied(), false);
        if self.carries_uninhabited(axis) {
            self.uninhabited = true;
        }
    }

    /// Show every bucket of `axis`, and the systems nobody lives in where
    /// the axis is the political one drawn
    pub fn show_all(&mut self, axis: ColorBy) {
        self.hidden[axis.slot()] = 0;
        if self.carries_uninhabited(axis) {
            self.uninhabited = false;
        }
    }

    /// Whether `axis`'s row carries the uninhabited chip: the color row,
    /// where it colors by a political axis
    pub fn carries_uninhabited(&self, axis: ColorBy) -> bool {
        Some(axis) == self.drawn && !axis.every_system()
    }

    /// Hide every bucket of `axis`, leaving the systems nobody lives in as
    /// they were
    ///
    /// They are not a value of the axis, and "none" of the governments is not
    /// a question about empty space.
    pub fn hide_all(&mut self, axis: ColorBy) {
        self.set(axis, 0..axis.buckets(), true);
    }

    /// Hide what `axis` shows and show what it hides
    pub fn invert(&mut self, axis: ColorBy) {
        let every = (1u32 << axis.buckets()) - 1;
        let bits = &mut self.hidden[axis.slot()];
        *bits = !*bits & every;
    }

    /// Hide or show the systems nobody lives in
    pub fn set_uninhabited(&mut self, hidden: bool) {
        self.uninhabited = hidden;
    }
}

/// What [`Mask::keeps`] leaves of one cell, by bucket of the axis being drawn
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Keeps {
    /// The drawn axis's hidden buckets
    hidden: u32,
}

/// What [`Mask::off_axis`] leaves of one cell, by share of each channel
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct OffAxis {
    /// The share of its colonies let through, `0.0..=1.0`: what the
    /// political axes not drawn leave of them, and star class's share too
    pub(crate) colonies: f32,
    /// The share of the systems nobody lives in let through, which only star
    /// class has a say over
    pub(crate) backdrop: f32,
}

impl OffAxis {
    /// Everything, for no axis but the drawn one asked
    pub(crate) const ALL: OffAxis = OffAxis { colonies: 1., backdrop: 1. };

    /// The share of a cell of `count` systems let through, `colonies` of
    /// them inhabited
    pub(crate) fn over(self, colonies: u64, count: u64) -> f32 {
        if count == 0 {
            return 1.;
        }
        let colonies = colonies.min(count);
        let alone = count - colonies;
        (colonies as f32 * self.colonies + alone as f32 * self.backdrop)
            / count as f32
    }
}

impl Keeps {
    /// Everything, for a mask cutting nothing
    pub(crate) const ALL: Keeps = Keeps { hidden: 0 };

    /// The share of `bucket`'s systems that is let through: all or none
    pub(crate) fn of(self, bucket: usize) -> f32 {
        match self.hidden & (1 << bucket) != 0 {
            true => 0.,
            false => 1.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mask over the map colored by allegiance, which is where the
    /// uninhabited flag means anything: the map's default is star class.
    fn colored_by_allegiance() -> Mask {
        let mut mask = Mask::default();
        mask.draw(Some(ColorBy::Allegiance));
        mask
    }

    /// The star a political test's systems arrive at, which no political
    /// axis asks about
    const NO_STAR: StarKind = StarKind::Unknown;

    fn federal(government: Government, security: Security) -> Buckets {
        Buckets::of(&Readings {
            allegiance: Some(Allegiance::Federation),
            government: Some(government),
            security: Some(security),
            ..Readings::default()
        })
    }

    fn bucket_of<T: Bucketed>(value: T) -> usize {
        T::bucket(Some(value))
    }

    /// A hidden allegiance is not let through, and the rest are
    #[test]
    fn a_hidden_allegiance_is_not_admitted() {
        let mut mask = Mask::default();
        mask.set(
            ColorBy::Allegiance,
            [bucket_of(Allegiance::Federation)],
            true,
        );
        assert!(!mask.admits(
            Some(federal(Government::Democracy, Security::High)),
            NO_STAR
        ));
        assert!(mask.admits(
            Some(Buckets::of(&Readings {
                allegiance: Some(Allegiance::Empire),
                government: Some(Government::Democracy),
                security: Some(Security::High),
                ..Readings::default()
            })),
            NO_STAR
        ));
        assert!(
            mask.admits(None, NO_STAR),
            "nobody lives there, so nothing hid it"
        );
    }

    /// Hiding a government goes on hiding it while the map is colored by
    /// security, its row standing beside the color row to say so
    #[test]
    fn every_axis_hiding_something_applies() {
        let mut mask = Mask::default();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        let prison = federal(Government::Prison, Security::High);

        mask.draw(Some(ColorBy::Security));
        assert!(!mask.admits(Some(prison), NO_STAR));
        assert!(mask.narrows(), "a hidden prison stopped narrowing");
        assert!(mask.admits(
            Some(federal(Government::Democracy, Security::High)),
            NO_STAR
        ));
    }

    /// Two axes are both: a system hidden along either is hidden, and one
    /// hidden along neither is let through
    #[test]
    fn two_axes_hide_what_either_hides() {
        let mut mask = Mask::default();
        mask.draw(Some(ColorBy::Security));
        mask.set(ColorBy::Security, [bucket_of(Security::High)], true);
        mask.draw(Some(ColorBy::State));
        mask.set(ColorBy::State, [bucket_of(State::Expansion)], true);
        let at = |security, state| {
            Buckets::of(&Readings {
                allegiance: Some(Allegiance::Federation),
                security: Some(security),
                state: Some(state),
                ..Readings::default()
            })
        };

        assert!(!mask.admits(Some(at(Security::High, State::Boom)), NO_STAR));
        assert!(
            !mask.admits(Some(at(Security::Low, State::Expansion)), NO_STAR)
        );
        assert!(
            !mask.admits(Some(at(Security::High, State::Expansion)), NO_STAR)
        );
        assert!(mask.admits(Some(at(Security::Low, State::Boom)), NO_STAR));
    }

    /// Two values of one axis are either: hiding all but two lets both
    /// through
    #[test]
    fn two_values_of_one_axis_are_either() {
        let mut mask = Mask::default();
        let (high, medium) =
            (bucket_of(Security::High), bucket_of(Security::Medium));
        mask.draw(Some(ColorBy::Allegiance));
        mask.set(ColorBy::Security, 0..Security::BUCKETS, true);
        mask.set(ColorBy::Security, [high, medium], false);
        let at = |security| federal(Government::Democracy, security);

        assert!(mask.admits(Some(at(Security::High)), NO_STAR));
        assert!(mask.admits(Some(at(Security::Medium)), NO_STAR));
        assert!(!mask.admits(Some(at(Security::Low)), NO_STAR));
    }

    /// A political axis not drawn says nothing of the systems nobody lives
    /// in, which have no reading on it, and the uninhabited flag says nothing
    /// without a political axis drawn to carry it
    #[test]
    fn a_row_off_the_axis_drawn_leaves_empty_space_alone() {
        let mut mask = Mask::default();
        mask.draw(Some(ColorBy::State));
        mask.set(ColorBy::Security, [bucket_of(Security::High)], true);
        assert!(mask.admits(None, NO_STAR), "empty space is not High");

        mask.set_uninhabited(true);
        assert!(!mask.admits(None, NO_STAR));
        mask.draw(Some(ColorBy::StarClass));
        assert!(mask.admits(None, NO_STAR), "star class carries no chip");
    }

    /// Star class is not asked while the sky is read as populations, whose
    /// colonies carry no star, and is kept for when it is not
    #[test]
    fn star_class_is_suspended_while_only_colonies_are_drawn() {
        let mut mask = Mask::default();
        let m = StarKind::M;
        mask.set(ColorBy::StarClass, [usize::from(m.code())], true);
        mask.draw(Some(ColorBy::Allegiance));
        let colony = Some(Buckets::of(&Readings::default()));
        assert!(!mask.admits(colony, m), "a colony at an M dwarf");
        assert!(!mask.suspended(ColorBy::StarClass));

        mask.draw_uninhabited(false);
        assert!(mask.admits(colony, StarKind::Unknown));
        assert!(!mask.narrows());
        assert!(mask.suspended(ColorBy::StarClass));
        assert!(mask.hiding(ColorBy::StarClass), "the row was forgotten");
    }

    /// A row off the axis drawn shows itself whole, or solos a value, and
    /// leaves the uninhabited flag to the color row whose chip it is
    #[test]
    fn a_row_off_the_axis_drawn_leaves_the_uninhabited_flag() {
        let mut mask = Mask::default();
        mask.draw(Some(ColorBy::State));
        mask.solo(ColorBy::Security, &[bucket_of(Security::High)]);
        assert!(!mask.hides_uninhabited());

        mask.set_uninhabited(true);
        mask.show_all(ColorBy::Security);
        assert!(!mask.hiding(ColorBy::Security));
        assert!(mask.hides_uninhabited());

        mask.show_all(ColorBy::State);
        assert!(!mask.hides_uninhabited(), "the color row's close");
    }

    /// A cell keeps what each axis not drawn lets through of it, the shares
    /// multiplied, and its backdrop only what star class does
    #[test]
    fn a_cell_keeps_the_product_of_the_shares_off_the_axis_drawn() {
        use galos_index::read::inhabited::Inhabited;
        let colony = |security, state| {
            Inhabited::of_system(
                [0.; 3],
                Readings { security, state, ..Readings::default() },
            )
        };
        let cell = colony(Some(Security::High), Some(State::Boom))
            .merge(colony(Some(Security::Low), Some(State::Boom)))
            .merge(colony(Some(Security::Low), Some(State::Expansion)))
            .merge(colony(Some(Security::Low), Some(State::War)));
        let mut mask = Mask::default();
        mask.draw(Some(ColorBy::Allegiance));
        assert_eq!(mask.off_axis(Some(&cell), None), OffAxis::ALL);

        mask.set(ColorBy::Security, [bucket_of(Security::High)], true);
        mask.set(
            ColorBy::State,
            [bucket_of(State::Expansion), bucket_of(State::War)],
            true,
        );
        let off = mask.off_axis(Some(&cell), None);
        assert_eq!(off.colonies, 0.375, "security's 3/4 of state's half");
        assert_eq!(off.backdrop, 1.);

        // The axis drawn is the key's to keep, not a share.
        mask.draw(Some(ColorBy::State));
        assert_eq!(mask.off_axis(Some(&cell), None).colonies, 0.75);

        let mut stars = [0; StarKind::COUNT];
        stars[usize::from(StarKind::M.code())] = 3;
        stars[usize::from(StarKind::G.code())] = 1;
        mask.set(ColorBy::StarClass, [usize::from(StarKind::M.code())], true);
        let off = mask.off_axis(Some(&cell), Some(&stars));
        assert_eq!((off.colonies, off.backdrop), (0.1875, 0.25));
        assert_eq!(off.over(1, 4), 0.234375);
    }

    /// What a cell keeps of its colonies comes to the same whichever of the
    /// axes hiding something the map is colored by
    ///
    /// Three axes keeping a quarter, a half and three quarters. Taking the
    /// smallest of the axes not drawn came to 3/16 colored by security and
    /// 1/8 colored by allegiance, so recoloring dimmed the far sky.
    #[test]
    fn a_cell_keeps_the_same_share_whichever_axis_is_drawn() {
        use galos_index::read::inhabited::Inhabited;
        let colony = |allegiance, security, state| {
            Inhabited::of_system(
                [0.; 3],
                Readings {
                    allegiance: Some(allegiance),
                    security: Some(security),
                    state: Some(state),
                    ..Readings::default()
                },
            )
        };
        let cell = colony(Allegiance::Federation, Security::High, State::Boom)
            .merge(colony(Allegiance::Empire, Security::Low, State::Boom))
            .merge(colony(Allegiance::Federation, Security::Low, State::War))
            .merge(colony(Allegiance::Federation, Security::Low, State::War));
        let mut mask = Mask::default();
        mask.set(
            ColorBy::Allegiance,
            [bucket_of(Allegiance::Federation)],
            true,
        );
        mask.set(ColorBy::Security, [bucket_of(Security::High)], true);
        mask.set(ColorBy::State, [bucket_of(State::Boom)], true);

        let kept = |mask: &mut Mask, drawn: ColorBy| {
            mask.draw(Some(drawn));
            let counts = drawn.counts(&cell);
            let whole: u32 = counts.iter().sum();
            let keeps = mask.keeps(drawn);
            let on_axis = counts
                .iter()
                .enumerate()
                .map(|(bucket, count)| *count as f32 * keeps.of(bucket))
                .sum::<f32>()
                / whole as f32;
            on_axis * mask.off_axis(Some(&cell), None).colonies
        };
        for drawn in [ColorBy::Allegiance, ColorBy::Security, ColorBy::State] {
            assert_eq!(kept(&mut mask, drawn), 0.09375, "colored by {drawn:?}");
        }
    }

    /// Star class asks every system's star, the systems nobody lives in
    /// among them, and the uninhabited flag says nothing along it
    #[test]
    fn star_class_asks_every_systems_star() {
        let mut mask = Mask::default();
        mask.set_uninhabited(true);
        mask.set(ColorBy::StarClass, [usize::from(StarKind::M.code())], true);
        mask.draw(Some(ColorBy::StarClass));

        assert!(!mask.admits(None, StarKind::M), "an empty M dwarf");
        assert!(mask.admits(None, StarKind::G), "an empty G star");
        let colony = Buckets::of(&Readings::default());
        assert!(!mask.admits(Some(colony), StarKind::M), "a colony at an M");
        assert_eq!(mask.keeps_uninhabited(), 1.);

        // And soloing a star leaves empty space as the allegiances had it.
        mask.set_uninhabited(false);
        mask.solo(ColorBy::StarClass, &[usize::from(StarKind::K.code())]);
        assert!(!mask.hides_uninhabited());
        assert!(mask.admits(None, StarKind::K));
        assert!(!mask.admits(None, StarKind::G));
    }

    /// Uninhabited applies whichever political axis is drawn, being a value
    /// of none
    #[test]
    fn uninhabited_applies_along_every_political_axis() {
        let mut mask = Mask::default();
        mask.set_uninhabited(true);
        for axis in ColorBy::POLITICAL {
            mask.draw(Some(axis));
            assert!(
                !mask.admits(None, NO_STAR),
                "{axis:?} let empty space through"
            );
        }
    }

    /// Anything hidden along any axis, or empty space hidden, narrows
    #[test]
    fn anything_hidden_narrows() {
        let mut mask = colored_by_allegiance();
        assert!(!mask.narrows());
        mask.set_uninhabited(true);
        assert!(mask.narrows());
        mask.set_uninhabited(false);
        mask.set(ColorBy::Security, [bucket_of(Security::High)], true);
        assert!(mask.narrows(), "security did not narrow an allegiance map");
    }

    /// Uninhabited hides the systems nobody lives in and no others, an
    /// unreported colony included
    #[test]
    fn uninhabited_hides_only_the_systems_nobody_lives_in() {
        let mut mask = colored_by_allegiance();
        mask.set_uninhabited(true);
        assert!(!mask.admits(None, NO_STAR));
        assert!(mask.admits(Some(Buckets::of(&Readings::default())), NO_STAR));
    }

    /// Solo shows one value of an axis and hides the rest, empty space
    /// included, and leaves the other axes as they were
    #[test]
    fn solo_shows_one_value_and_leaves_the_other_axes() {
        let mut mask = colored_by_allegiance();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        let federation = bucket_of(Allegiance::Federation);
        mask.solo(ColorBy::Allegiance, &[federation]);
        for bucket in 0..Allegiance::BUCKETS {
            assert_eq!(
                mask.hides(ColorBy::Allegiance, bucket),
                bucket != federation
            );
        }
        assert!(mask.hides_uninhabited());
        assert!(mask.hides(ColorBy::Government, bucket_of(Government::Prison)));
    }

    /// Toggling a group partly hidden hides the rest of it; toggling it
    /// again shows the lot
    #[test]
    fn a_partly_hidden_group_is_hidden_the_rest_of_the_way() {
        let mut mask = Mask::default();
        let red =
            [bucket_of(Government::Prison), bucket_of(Government::Feudal)];
        mask.set(ColorBy::Government, [red[0]], true);
        mask.toggle(ColorBy::Government, &red);
        assert!(red.iter().all(|b| mask.hides(ColorBy::Government, *b)));
        mask.toggle(ColorBy::Government, &red);
        assert!(red.iter().all(|b| !mask.hides(ColorBy::Government, *b)));
    }

    /// None hides every value of an axis and leaves empty space alone; all
    /// shows both; invert stays inside the axis
    #[test]
    fn the_footer_answers_one_axis() {
        let mut mask = Mask::default();
        mask.draw(Some(ColorBy::Security));
        mask.hide_all(ColorBy::Security);
        assert!(
            (0..Security::BUCKETS).all(|b| mask.hides(ColorBy::Security, b))
        );
        assert!(!mask.hides_uninhabited());
        mask.invert(ColorBy::Security);
        assert!(!mask.narrows());
        mask.set_uninhabited(true);
        mask.hide_all(ColorBy::Security);
        mask.show_all(ColorBy::Security);
        assert!(!mask.narrows());
    }

    /// A cell keeps every bucket of the axis drawn but the hidden ones, and
    /// the other axes' hidden buckets take no bucket out of it: those are
    /// [`Mask::off_axis`]'s shares
    #[test]
    fn a_cell_keeps_all_but_the_hidden_buckets_of_the_axis_drawn() {
        let mut mask = Mask::default();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        mask.set(ColorBy::Security, [bucket_of(Security::Low)], true);
        mask.draw(Some(ColorBy::Government));

        let drawn = mask.keeps(ColorBy::Government);
        assert_eq!(drawn.of(bucket_of(Government::Prison)), 0.);
        assert_eq!(drawn.of(bucket_of(Government::Democracy)), 1.);

        // A histogram read along another axis than the mask asks keeps all
        // of itself: the frame before the mask follows the coloring.
        assert_eq!(mask.keeps(ColorBy::Security), Keeps::ALL);
    }

    /// Not colored at all, every axis hiding something still applies, each
    /// standing as its row, and the uninhabited flag is kept for the color
    /// row that carries it: colored again, it hides what it hid
    #[test]
    fn an_uncolored_map_keeps_its_rows_and_the_flag() {
        let mut mask = Mask::default();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        mask.set_uninhabited(true);
        let prison = federal(Government::Prison, Security::High);

        mask.draw(None);
        assert!(mask.narrows());
        assert!(!mask.admits(Some(prison), NO_STAR));
        assert!(mask.admits(None, NO_STAR));
        assert_eq!(mask.keeps_uninhabited(), 1.);
        assert_eq!(mask.keeps(ColorBy::Government), Keeps::ALL);

        mask.draw(Some(ColorBy::Government));
        assert!(
            !mask.admits(Some(prison), NO_STAR) && !mask.admits(None, NO_STAR)
        );
    }

    /// A map drawing no uninhabited system ignores the flag hiding them, and
    /// keeps it for when it draws them again; the colors still apply
    #[test]
    fn uninhabited_is_ignored_where_none_is_drawn() {
        let mut mask = colored_by_allegiance();
        mask.set_uninhabited(true);
        mask.draw_uninhabited(false);
        assert!(!mask.narrows());
        assert!(mask.admits(None, NO_STAR));
        assert_eq!(mask.keeps_uninhabited(), 1.);
        assert!(mask.hides_uninhabited(), "the flag was forgotten");

        mask.set(
            ColorBy::Allegiance,
            [bucket_of(Allegiance::Federation)],
            true,
        );
        assert!(mask.narrows(), "a hidden color stopped hiding");

        mask.draw_uninhabited(true);
        assert!(!mask.admits(None, NO_STAR));
    }
}
