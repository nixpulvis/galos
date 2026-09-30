//! What the user has asked not to see, by the color a system is drawn in
//!
//! Every color of every political axis is a toggle, and a toggled one hides
//! the systems drawn in it. The mask is kept on the index's own buckets
//! ([`Bucketed`]) rather than on [`Hue`]: seven governments share red, and each
//! has to be hidden on its own.
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
//! A new axis is a variant of [`ColorBy`] and an arm in each `match` below,
//! and a layout in [`super::key`]: the mask, the key and the field all walk
//! [`ColorBy::ALL`], so none of them has a list of its own to keep up.

use crate::map::galaxy::spawn::{ColorBy, Hue};
use elite_journal::{Allegiance, Government, system::Security};
use galos_index::read::inhabited::{Bucketed, Inhabited};
use galos_index::records::PopulatedSystem;

/// A system's political readings, by the bucket each counts in
///
/// What the mask asks about a system, in the index's own terms, so a system
/// on the map, a payload point joined against the populated table and a cell's
/// histogram are all read by the one numbering. [`None`] where a candidate
/// carries one of these is a system nobody lives in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Buckets {
    allegiance: u8,
    government: u8,
    security: u8,
}

impl Buckets {
    /// The buckets three readings count in
    pub(crate) fn of(
        allegiance: Option<Allegiance>,
        government: Option<Government>,
        security: Option<Security>,
    ) -> Buckets {
        // The largest axis has eighteen buckets, which a byte holds.
        Buckets {
            allegiance: Allegiance::bucket(allegiance) as u8,
            government: Government::bucket(government) as u8,
            security: Security::bucket(security) as u8,
        }
    }

    /// The buckets a populated table's row counts in, where anybody lives there
    ///
    /// Inhabited is `population > 0`, exactly as [`Inhabited`] counts it, so a
    /// row the histograms leave out is one the mask reads as uninhabited too
    /// and the field and the marks cannot disagree about which it is.
    pub(crate) fn of_row(row: &PopulatedSystem) -> Option<Buckets> {
        (row.population > 0)
            .then(|| Buckets::of(row.allegiance, row.government, row.security))
    }

    /// The bucket this system counts in along `axis`
    pub(crate) fn on(self, axis: ColorBy) -> usize {
        usize::from(match axis {
            ColorBy::Allegiance => self.allegiance,
            ColorBy::Government => self.government,
            ColorBy::Security => self.security,
        })
    }
}

impl ColorBy {
    /// Every axis the map can be colored by, in the order the key tabs them
    pub const ALL: [ColorBy; 3] =
        [ColorBy::Allegiance, ColorBy::Government, ColorBy::Security];

    /// Where this axis stands in [`Self::ALL`], which is where its bits are
    /// kept in a [`Mask`]
    fn slot(self) -> usize {
        match self {
            ColorBy::Allegiance => 0,
            ColorBy::Government => 1,
            ColorBy::Security => 2,
        }
    }

    /// How many buckets this axis counts in, the unreported one included
    pub fn buckets(self) -> usize {
        match self {
            ColorBy::Allegiance => Allegiance::BUCKETS,
            ColorBy::Government => Government::BUCKETS,
            ColorBy::Security => Security::BUCKETS,
        }
    }

    /// The color a bucket of this axis is drawn in
    ///
    /// Through the very mapping a mark is painted by, so the key and the map
    /// cannot disagree about what a color means.
    pub(crate) fn hue_of(self, bucket: usize) -> Hue {
        match self {
            ColorBy::Allegiance => Hue::allegiance(Bucketed::at(bucket)),
            ColorBy::Government => Hue::government(Bucketed::at(bucket)),
            ColorBy::Security => Hue::security(Bucketed::at(bucket)),
        }
    }

    /// A cell's colonies counted along this axis, bucket by bucket
    pub(crate) fn counts(self, held: &Inhabited) -> &[u32] {
        match self {
            ColorBy::Allegiance => held.allegiance(),
            ColorBy::Government => held.government(),
            ColorBy::Security => held.security(),
        }
    }

    /// What the axis is called, as a tab or a row says it
    pub fn name(self) -> &'static str {
        match self {
            ColorBy::Allegiance => "Allegiance",
            ColorBy::Government => "Government",
            ColorBy::Security => "Security",
        }
    }
}

/// Which buckets of every axis are hidden, which axis that is asked along,
/// and whether it is being applied
///
/// A bit a bucket, one word an axis: the largest axis is eighteen buckets.
/// Only the words of the axis the map is colored by ([`Self::drawn`]) are
/// asked; the others are kept for when the map is colored by theirs.
///
/// Off without being forgotten, as a filter's row is: [`Self::enabled`] false
/// admits everything the bits would hide and keeps the bits, so the mask can
/// be lifted to see what it was hiding and put back with one click.
///
/// Edited through [`crate::map::filter::Filters::edit_mask`] and nowhere else,
/// which is what counts an edit as a change to what the filters admit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    hidden: [u32; ColorBy::ALL.len()],
    /// The axis the map is colored by, which is the one asked
    ///
    /// A copy of [`ColorBy`], kept here because every pass that asks the
    /// mask asks it about a system and not about the map, and is followed
    /// from the resource by [`crate::map::filter::follow_color_by`].
    drawn: ColorBy,
    /// Whether the systems nobody lives in are hidden
    ///
    /// One flag shared by every axis: an uninhabited system has no reading on
    /// any of them, so there is nothing to tell apart.
    uninhabited: bool,
    enabled: bool,
}

impl Default for Mask {
    /// Hiding nothing, and on: a mask is applied as soon as anything is set
    /// in it, a chip being a one-click toggle.
    fn default() -> Mask {
        Mask {
            hidden: [0; ColorBy::ALL.len()],
            drawn: ColorBy::Allegiance,
            uninhabited: false,
            enabled: true,
        }
    }
}

impl Mask {
    /// Whether what is set is being applied
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether `bucket` of `axis` is set to be hidden, applied or not
    pub fn hides(&self, axis: ColorBy, bucket: usize) -> bool {
        self.hidden[axis.slot()] & (1 << bucket) != 0
    }

    /// Whether the systems nobody lives in are set to be hidden, applied or
    /// not
    pub fn hides_uninhabited(&self) -> bool {
        self.uninhabited
    }

    /// Whether anything is set that the axis drawn would hide, applied or
    /// not
    pub fn hides_anything(&self) -> bool {
        self.uninhabited || self.hidden[self.drawn.slot()] != 0
    }

    /// Whether the mask is cutting anything off the map
    pub(crate) fn narrows(&self) -> bool {
        self.enabled && self.hides_anything()
    }

    /// The axis the mask is asked along
    pub fn drawn(&self) -> ColorBy {
        self.drawn
    }

    /// Ask along `axis` from here on, keeping what every axis hides
    pub(crate) fn draw(&mut self, axis: ColorBy) {
        self.drawn = axis;
    }

    /// Whether a system reading `politics` is let through
    ///
    /// [`None`] is a system nobody lives in, which only the uninhabited flag
    /// says anything about.
    pub(crate) fn admits(&self, politics: Option<Buckets>) -> bool {
        if !self.enabled {
            return true;
        }
        match politics {
            None => !self.uninhabited,
            Some(buckets) => !self.hides(self.drawn, buckets.on(self.drawn)),
        }
    }

    /// Which of a cell's buckets along `drawn` the mask lets through
    ///
    /// Exact, the mask being asked along the axis the histogram is read
    /// along. Everything where `drawn` is not the axis the mask asks, which
    /// is a frame between the coloring changing and the mask following it.
    pub(crate) fn keeps(&self, drawn: ColorBy) -> Keeps {
        if !self.narrows() || drawn != self.drawn {
            return Keeps::ALL;
        }
        Keeps { hidden: self.hidden[drawn.slot()] }
    }

    /// The share of the systems nobody lives in that is let through: all of
    /// them or none
    pub(crate) fn keeps_uninhabited(&self) -> f32 {
        match self.enabled && self.uninhabited {
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
    /// lives in included
    ///
    /// The other axes are left as they were. Soloing an allegiance is asking
    /// to see only it, not to forget which governments were hidden.
    pub fn solo(&mut self, axis: ColorBy, buckets: &[usize]) {
        self.set(axis, 0..axis.buckets(), true);
        self.set(axis, buckets.iter().copied(), false);
        self.uninhabited = true;
        self.enabled = true;
    }

    /// Show every bucket of `axis`, and the systems nobody lives in
    pub fn show_all(&mut self, axis: ColorBy) {
        self.hidden[axis.slot()] = 0;
        self.uninhabited = false;
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

    /// Apply what is set, or lift it without forgetting it
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
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

    fn federal(government: Government, security: Security) -> Buckets {
        Buckets::of(
            Some(Allegiance::Federation),
            Some(government),
            Some(security),
        )
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
        assert!(
            !mask.admits(Some(federal(Government::Democracy, Security::High)))
        );
        assert!(mask.admits(Some(Buckets::of(
            Some(Allegiance::Empire),
            Some(Government::Democracy),
            Some(Security::High),
        ))));
        assert!(mask.admits(None), "nobody lives there, so nothing hid it");
    }

    /// Hiding a government hides nothing while the map is colored by
    /// security, and hides it again once it is colored by government
    #[test]
    fn only_the_axis_drawn_applies() {
        let mut mask = Mask::default();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        let prison = federal(Government::Prison, Security::High);

        mask.draw(ColorBy::Security);
        assert!(mask.admits(Some(prison)));
        assert!(!mask.narrows(), "a hidden prison narrowed a security map");

        mask.draw(ColorBy::Government);
        assert!(!mask.admits(Some(prison)));
        assert!(
            mask.admits(Some(federal(Government::Democracy, Security::High)))
        );
    }

    /// Uninhabited applies whichever axis is drawn, being a value of none
    #[test]
    fn uninhabited_applies_along_every_axis() {
        let mut mask = Mask::default();
        mask.set_uninhabited(true);
        for axis in ColorBy::ALL {
            mask.draw(axis);
            assert!(!mask.admits(None), "{axis:?} let empty space through");
        }
    }

    /// Lifted, the mask lets everything through and keeps what was set
    #[test]
    fn a_lifted_mask_admits_everything_and_forgets_nothing() {
        let mut mask = Mask::default();
        mask.set(
            ColorBy::Allegiance,
            [bucket_of(Allegiance::Federation)],
            true,
        );
        mask.set_uninhabited(true);
        mask.set_enabled(false);
        assert!(
            mask.admits(Some(federal(Government::Democracy, Security::High)))
        );
        assert!(mask.admits(None));
        assert!(!mask.narrows());
        mask.set_enabled(true);
        assert!(
            !mask.admits(Some(federal(Government::Democracy, Security::High)))
        );
        assert!(!mask.admits(None));
    }

    /// Uninhabited hides the systems nobody lives in and no others, an
    /// unreported colony included
    #[test]
    fn uninhabited_hides_only_the_systems_nobody_lives_in() {
        let mut mask = Mask::default();
        mask.set_uninhabited(true);
        assert!(!mask.admits(None));
        assert!(mask.admits(Some(Buckets::of(None, None, None))));
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
        mask.draw(ColorBy::Security);
        mask.hide_all(ColorBy::Security);
        assert!(
            (0..Security::BUCKETS).all(|b| mask.hides(ColorBy::Security, b))
        );
        assert!(!mask.hides_uninhabited());
        mask.invert(ColorBy::Security);
        assert!(!mask.hides_anything());
        mask.set_uninhabited(true);
        mask.hide_all(ColorBy::Security);
        mask.show_all(ColorBy::Security);
        assert!(!mask.hides_anything());
    }

    /// A cell keeps every bucket of the axis drawn but the hidden ones, and
    /// the other axes' hidden buckets take nothing out of it
    #[test]
    fn a_cell_keeps_all_but_the_hidden_buckets_of_the_axis_drawn() {
        let mut mask = Mask::default();
        mask.set(ColorBy::Government, [bucket_of(Government::Prison)], true);
        mask.set(ColorBy::Security, [bucket_of(Security::Low)], true);
        mask.draw(ColorBy::Government);

        let drawn = mask.keeps(ColorBy::Government);
        assert_eq!(drawn.of(bucket_of(Government::Prison)), 0.);
        assert_eq!(drawn.of(bucket_of(Government::Democracy)), 1.);

        // A histogram read along another axis than the mask asks keeps all
        // of itself: the frame before the mask follows the coloring.
        assert_eq!(mask.keeps(ColorBy::Security), Keeps::ALL);

        mask.set_enabled(false);
        assert_eq!(mask.keeps(ColorBy::Government), Keeps::ALL);
    }
}
