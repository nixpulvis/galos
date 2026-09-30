//! What the user has asked not to see, by the color a system is drawn in
//!
//! Every color of every axis is a toggle, and a toggled one hides the systems
//! drawn in it. The mask is kept on the index's own buckets ([`Bucketed`])
//! rather than on [`Hue`]: seven governments share red, and each has to be
//! hidden on its own.
//!
//! **Only the axis the map is colored by applies.** Hiding the prisons and
//! then coloring by security shows every security rating, the prisons among
//! them: the key is read as a key to the colors on screen, and a system
//! missing for a color that is not on screen is a system missing for no
//! reason the reader can see. What each axis hides is remembered, so coloring
//! by government again hides the prisons again. Uninhabited is the one
//! exception, being no value of any axis: it applies whichever is drawn.
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
    /// Every axis the map can be colored by, in the order the key tabs them
    pub const ALL: [ColorBy; 8] = [
        ColorBy::Allegiance,
        ColorBy::Government,
        ColorBy::Security,
        ColorBy::Economy,
        ColorBy::State,
        ColorBy::Power,
        ColorBy::PowerplayState,
        ColorBy::StarClass,
    ];

    /// The axes read off a colony's populated columns, which are the first
    /// of [`Self::ALL`] and the ones a [`Buckets`] holds
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

    /// Where this axis stands in [`Self::ALL`], which is where its bits are
    /// kept in a [`Mask`] and, for a political axis, its bucket in a
    /// [`Buckets`]
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

    /// What the axis is called, as a tab or a row says it
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

/// Which buckets of every axis are hidden, and which axis that is asked along
///
/// A bit a bucket, one word an axis: the largest axis is twenty-eight buckets.
/// Only the words of the axis the map is colored by ([`Self::drawn`]) are
/// asked; the others are kept for when the map is colored by theirs.
///
/// Always applied. There is no lifting it as a filter's row is lifted: a
/// color shown is a chip clicked back on, and with only the axis on screen
/// asked there is nothing out of sight for a switch to bring back.
///
/// Edited through [`crate::map::filter::Filters::edit_mask`] and nowhere else,
/// which is what counts an edit as a change to what the filters admit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    hidden: [u32; ColorBy::ALL.len()],
    /// The axis the map is colored by, which is the one asked, and nothing
    /// in the realistic view, which colors the stars by their own light
    ///
    /// A copy of [`ColorBy`] and the view, kept here because every pass that
    /// asks the mask asks it about a system and not about the map, and
    /// followed from the resources by [`crate::map::filter::follow_color_by`].
    /// Nothing drawn in a color is nothing a color key can hide: the mask
    /// lets everything through, and keeps what it holds for the map view.
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
    /// and kept, as the whole mask is in the realistic view. Followed from
    /// the settings by [`crate::map::filter::follow_color_by`].
    empty_drawn: bool,
}

impl Default for Mask {
    /// Hiding nothing.
    fn default() -> Mask {
        Mask {
            hidden: [0; ColorBy::ALL.len()],
            drawn: Some(ColorBy::Allegiance),
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
    /// Never along an axis that colors every system, which has no
    /// uninhabited systems to set apart.
    pub(crate) fn hides_empty(&self) -> bool {
        self.uninhabited
            && self.empty_drawn
            && !self.drawn.is_some_and(ColorBy::every_system)
    }

    /// Whether the mask is cutting anything off the map: anything hidden
    /// along the axis drawn, or the systems nobody lives in where they are
    /// drawn, while the map is colored at all
    pub(crate) fn narrows(&self) -> bool {
        self.drawn.is_some_and(|drawn| {
            self.hides_empty() || self.hidden[drawn.slot()] != 0
        })
    }

    /// The axis the mask is asked along, nothing in the realistic view
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
    /// [`None`] is a system nobody lives in, which along a political axis
    /// only the uninhabited flag says anything about. Star class asks the
    /// kind of every system alike.
    pub(crate) fn admits(
        &self,
        politics: Option<Buckets>,
        kind: StarKind,
    ) -> bool {
        let Some(drawn) = self.drawn else { return true };
        if drawn.every_system() {
            return !self.hides(drawn, usize::from(kind.code()));
        }
        match politics {
            None => !self.hides_empty(),
            Some(buckets) => !self.hides(drawn, buckets.on(drawn)),
        }
    }

    /// Which of a cell's buckets along `drawn` the mask lets through
    ///
    /// Exact, the mask being asked along the axis the histogram is read
    /// along. Everything where `drawn` is not the axis the mask asks, which
    /// is a frame between the coloring changing and the mask following it.
    pub(crate) fn keeps(&self, drawn: ColorBy) -> Keeps {
        if !self.narrows() || Some(drawn) != self.drawn {
            return Keeps::ALL;
        }
        Keeps { hidden: self.hidden[drawn.slot()] }
    }

    /// The share of the systems nobody lives in that is let through: all of
    /// them or none
    pub(crate) fn keeps_uninhabited(&self) -> f32 {
        match self.narrows() && self.hides_empty() {
            true => 0.,
            false => 1.,
        }
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
    /// lives in included where the axis is a political one
    ///
    /// The other axes are left as they were. Soloing an allegiance is asking
    /// to see only it, not to forget which governments were hidden; and
    /// soloing a star is not asking to hide empty space from the allegiances.
    pub fn solo(&mut self, axis: ColorBy, buckets: &[usize]) {
        self.set(axis, 0..axis.buckets(), true);
        self.set(axis, buckets.iter().copied(), false);
        if !axis.every_system() {
            self.uninhabited = true;
        }
    }

    /// Show every bucket of `axis`, and the systems nobody lives in where
    /// the axis is a political one
    pub fn show_all(&mut self, axis: ColorBy) {
        self.hidden[axis.slot()] = 0;
        if !axis.every_system() {
            self.uninhabited = false;
        }
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

    /// Hiding a government hides nothing while the map is colored by
    /// security, and hides it again once it is colored by government
    #[test]
    fn only_the_axis_drawn_applies() {
        let mut mask = Mask::default();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        let prison = federal(Government::Prison, Security::High);

        mask.draw(Some(ColorBy::Security));
        assert!(mask.admits(Some(prison), NO_STAR));
        assert!(!mask.narrows(), "a hidden prison narrowed a security map");

        mask.draw(Some(ColorBy::Government));
        assert!(!mask.admits(Some(prison), NO_STAR));
        assert!(mask.admits(
            Some(federal(Government::Democracy, Security::High)),
            NO_STAR
        ));
    }

    /// A hidden state hides the systems in it once the map is colored by
    /// state, and the other readings of those systems do not come into it
    #[test]
    fn a_hidden_state_hides_its_systems_along_state() {
        let mut mask = Mask::default();
        mask.set(ColorBy::State, [bucket_of(State::War)], true);
        let at = |state| {
            Buckets::of(&Readings {
                allegiance: Some(Allegiance::Federation),
                state: Some(state),
                ..Readings::default()
            })
        };

        assert!(
            mask.admits(Some(at(State::War)), NO_STAR),
            "drawn by allegiance"
        );
        mask.draw(Some(ColorBy::State));
        assert!(!mask.admits(Some(at(State::War)), NO_STAR));
        assert!(mask.admits(Some(at(State::Boom)), NO_STAR));
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

    /// Only what the axis drawn hides, or empty space hidden, narrows
    #[test]
    fn nothing_hidden_along_the_axis_drawn_narrows_nothing() {
        let mut mask = Mask::default();
        assert!(!mask.narrows());
        mask.set(ColorBy::Security, [bucket_of(Security::High)], true);
        assert!(!mask.narrows(), "security narrowed an allegiance map");
        mask.set_uninhabited(true);
        assert!(mask.narrows());
    }

    /// Uninhabited hides the systems nobody lives in and no others, an
    /// unreported colony included
    #[test]
    fn uninhabited_hides_only_the_systems_nobody_lives_in() {
        let mut mask = Mask::default();
        mask.set_uninhabited(true);
        assert!(!mask.admits(None, NO_STAR));
        assert!(mask.admits(Some(Buckets::of(&Readings::default())), NO_STAR));
    }

    /// Solo shows one value of an axis and hides the rest, empty space
    /// included, and leaves the other axes as they were
    #[test]
    fn solo_shows_one_value_and_leaves_the_other_axes() {
        let mut mask = Mask::default();
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
    /// the other axes' hidden buckets take nothing out of it
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

    /// Not colored at all, the mask lets everything through and forgets
    /// nothing: colored again, it hides what it hid
    #[test]
    fn an_uncolored_map_ignores_the_mask_and_keeps_it() {
        let mut mask = Mask::default();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        mask.set_uninhabited(true);
        let prison = federal(Government::Prison, Security::High);

        mask.draw(None);
        assert!(!mask.narrows());
        assert!(
            mask.admits(Some(prison), NO_STAR) && mask.admits(None, NO_STAR)
        );
        assert_eq!(mask.keeps_uninhabited(), 1.);

        mask.draw(Some(ColorBy::Government));
        assert!(
            !mask.admits(Some(prison), NO_STAR) && !mask.admits(None, NO_STAR)
        );
    }

    /// A map drawing no uninhabited system ignores the flag hiding them, and
    /// keeps it for when it draws them again; the colors still apply
    #[test]
    fn uninhabited_is_ignored_where_none_is_drawn() {
        let mut mask = Mask::default();
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
