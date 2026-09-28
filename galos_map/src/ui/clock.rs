//! When the map is standing: the reading at the top of the viewport, and the
//! scrubber that drops out under it
//!
//! The moment is the galaxy's rather than a system's, so it stands in no frame
//! of the bar's. See [`crate::ui`] for the three zones.

use crate::map::bodies::{Clock, Contents};
use crate::map::selection::Selection;
use crate::ui::text::thousands;
use crate::ui::widgets::{field_height, greyed};
use crate::ui::{ClockControl, Dropping, Standing};
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Context, Ui};
use chrono::Datelike;

/// Put the map back to the present, the reading of it having gone
///
/// What hiding the clock comes to besides drawing nothing: the offset is let
/// go of and the scrubber shut, so that turning the strip off is the map at
/// `now` and turning it back on is a strip that says so. Left standing, the
/// offset would go on being drawn into every orbit with nothing on screen to
/// say why the planets are where they are.
pub(super) fn hidden(clock: &mut Clock, control: &mut ClockControl) {
    clock.reset();
    control.out = false;
}

/// How wide the strip stands while the scrubber is out
///
/// A little wider than the reading it is opened from, which comes to some
/// 250 points with a span and a `Now` beside the date, and wide enough for
/// the spans [`marks`] writes under the rail without them running together.
/// It was drawn at 620 to begin with — nearly twice the bar — on the
/// argument that a logarithmic rail spends its decades over its own width, so
/// every pixel taken off it is a coarser instrument. True, and beside the
/// point: at that width the reading sat in a third of a box and the rail was
/// a bare grey bar across the top of the map. Exactness on the rail is not
/// what the far end of it is for, and the marks are what make a coarse rail
/// readable.
///
/// A number rather than what the reading leaves over. The rail is scaled by
/// the room it is in, and room measured off a line that grows with the value
/// the rail last set is a control whose scale is a function of its own value.
pub(super) const STRIP_WIDTH: f32 = 420.;

/// How wide the scrubber's rail runs
///
/// The strip's own width, which is what [`time_strip`] sets the row the rail
/// stands in to: the frame's margins are outside that, so the rail fills the
/// row rather than stopping short of it. A number for the reason
/// [`clock_control`] gives at length — a rail scaled by the room a growing
/// reading leaves is a rail scaled by its own value — so it is written down
/// beside the width it comes from rather than measured where it is used.
const RAIL_WIDTH: f32 = STRIP_WIDTH;

/// When the map is standing, at the top of the viewport
///
/// Its own zone, in the middle of the top edge. The moment is the galaxy's:
/// it is true whatever the bar is being asked and whether or not the camera is
/// inside a system, so it is read where a reading of that kind belongs rather
/// than filed inside the corner card that comes and goes. In the bar it moved
/// down the screen every time a form dropped out above it.
///
/// Bare while the map stands at the present, which is how it opens: a weak
/// line of text over the sky and nothing else. Clicking it drops the scrubber
/// out under it, and then it takes the frame — the same [`Dropping`] the bar
/// is drawn in, and put away by the same gestures: see [`Pane`](crate::ui::Pane).
///
/// Answers where it stood, which is what
/// `the_strip_gives_way_to_the_bar_on_a_narrow_window` reads. Change
/// detection is the caller's: what the scrubber writes is a moment, and
/// whether the clock moved has nothing to do with where the reading of it is
/// drawn.
pub(super) fn time_strip(
    ctx: &Context,
    chrome_right: f32,
    clock: &mut Clock,
    control: &mut ClockControl,
    turns: Turns,
) -> egui::Rect {
    Dropping {
        id: "time-strip",
        standing: Standing::Middle { beside: chrome_right },
        out: control.out,
        width: STRIP_WIDTH,
        holds_width: false,
    }
    .show(ctx, |ui| dated(ui, clock, turns, control))
    .response
    .rect
}

/// What the scrubber may be geared to
///
/// Both turns rather than the better of them, since which is wanted is the
/// reader's to say: a planet's own year is the span that says something about
/// the planet, and the system's widest orbit is the span in which the whole
/// arrangement has been through every shape it takes. See [`GearedTo`], which
/// is what the strip asks and this answers.
///
/// Either may be missing. There is no body's turn without a body picked out
/// in the system the map is holding, and no system's turn where no orbit in
/// it has a period on record.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct Turns {
    /// One turn of the body picked out
    body: Option<f64>,
    /// One turn of the widest orbit the system has on record
    system: Option<f64>,
}

impl Turns {
    /// What the rail covers, given which of the two is asked for
    ///
    /// The other where the one asked for is not on record, so that a rail is
    /// offered wherever there is any turn to cover: the choice is which of
    /// two spans to read, and it is nothing to do with whether the map can be
    /// run on at all.
    fn geared(self, to: GearedTo) -> Option<Geared> {
        let body = self.body.map(Geared::Body);
        let system = self.system.map(Geared::System);
        match to {
            GearedTo::Body => body.or(system),
            GearedTo::System => system.or(body),
        }
    }

    /// Whether there is a choice to offer
    ///
    /// Both, or there is nothing to choose between and a switch that reads as
    /// two ways of asking for the same rail.
    fn choice(self) -> bool {
        self.body.is_some() && self.system.is_some()
    }
}

/// Which turn the scrubber's rail is asked to cover
///
/// The body picked out by default, that being the narrower of the two and the
/// one the reader said something about by picking it: a rail over the whole
/// system's widest orbit moves a planet by whole years at a nudge.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) enum GearedTo {
    /// One turn of the body picked out, laid evenly
    #[default]
    Body,
    /// One turn of the system's widest orbit, laid by decades
    System,
}

impl GearedTo {
    /// The two, in the order the switch stands them in
    ///
    /// The narrower first, as the strip reads left to right and as the rails
    /// themselves run.
    const ALL: [GearedTo; 2] = [GearedTo::Body, GearedTo::System];

    /// What the switch calls it
    fn said(self) -> &'static str {
        match self {
            GearedTo::Body => "Body",
            GearedTo::System => "System",
        }
    }

    /// What choosing it does, said on hover. See [`check`](crate::ui::widgets::check).
    fn hint(self) -> &'static str {
        match self {
            GearedTo::Body => "Cover one turn of the body picked out",
            GearedTo::System => "Cover one turn of the system's widest orbit",
        }
    }
}

/// The turns the map has to offer, where the camera is standing
///
/// A body's turn is only a body's turn while the map is holding the system it
/// is in: what is picked out survives a flight and its period does not follow
/// it out of the system it was scanned in.
pub(super) fn turns_of(selection: &Selection, contents: &Contents) -> Turns {
    Turns {
        body: selection
            .newest_body()
            .filter(|(address, _)| contents.of() == Some(*address))
            .and_then(|(_, id)| contents.turn_of(id)),
        system: contents.slowest_turn(),
    }
}

/// Say what moment the map is standing at
///
/// Drawn in [`time_strip`], at the top of the viewport and in the middle of
/// it. What the map is doing, said where a reader is looking rather than kept
/// behind the gear, and its own zone rather than a line in the bar: the
/// moment is the galaxy's and holds whatever the bar is being asked, so a
/// reading filed under the asking moved down the screen every time a form
/// dropped out above it. It answers the one question the arrangement on
/// screen raises — when is this — and it is the only place the map ever says
/// what day the game is on.
///
/// The date alone while nothing has run the map on, which is how it opens. A
/// slider puts the map some span past the present, and then the span is named
/// beside the date and can be let go of: the sliders each cover one turn of
/// something, so none of them can reach back to nothing on its own.
///
/// Said whether or not a system is held. The moment is the galaxy's and not a
/// system's -- see [`Clock`] -- so there is a date to read out on the way
/// between two of them, and `Now` is reachable from wherever a drag was left.
/// It was drawn off the held system's newest scan to begin with, which meant
/// no line at all out in the sky and an offset that could be set from a
/// panel with no way to let go of it.
///
/// Clicking the reading opens the slider that sets it, in the line below.
/// What a reader wants to change is the thing they are reading, so the way to
/// it is the reading itself rather than a control filed away in the pane
/// where nobody would find it. The span past the present answers a click as
/// the date does, the two being halves of one moment. Clicked again the
/// slider goes, the map left wherever it put it -- `Now` is what lets go of
/// that.
fn dated(
    ui: &mut Ui,
    clock: &mut Clock,
    turns: Turns,
    control: &mut ClockControl,
) {
    let out = control.out;
    let running_on = clock.offset() != 0.;
    let clicked = ui
        // To the height the bar's box comes to, and its contents laid
        // level in it, so that the reading stands on the same line as the
        // field and the gear hung on the field's own middle. Both panes are
        // at the top of the viewport behind the same padding, so the one
        // thing that had them out of true was that a field is padded inside
        // and a line of text is not: the reading sat a few points high of
        // the box beside it. See [`field_height`].
        .allocate_ui_with_layout(
            egui::vec2(ui.available_width(), field_height(ui)),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let mut asked = reading(ui, drawn_at(clock));
                if running_on {
                    // In the marks' own words rather than
                    // [`crate::ui::panels::fields::lasting`]'s. The two stand in one
                    // line with the switch and `Now` at the end of it, and
                    // `+14989.7 Earth years` -- which is what the far end of a
                    // wide pair's rail comes to -- ran clean through them.
                    asked |= reading(
                        ui,
                        format!("+{}", briefly(clock.offset(), true)),
                    );
                }

                // The controls stand at the far end of the strip while the
                // scrubber is out, which is what fills a line the reading only
                // half covers, and is a place they keep: read beside the span
                // they are about, they walk along the line as it grows a digit.
                //
                // Beside the span while the strip is shut, there being no width
                // to stand at the end of: the strip is then only as wide as what
                // is written in it, and there is no rail to gear.
                let mut let_go = false;
                if out {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if running_on {
                                let_go = ui.small_button("Now").clicked();
                            }
                            // Beside `Now` at the far end rather than beside the
                            // reading: the span in the reading grows and shrinks
                            // as the rail is dragged, and a switch that slid
                            // along above the rail with it would be a control
                            // moving under the hand using it.
                            if turns.choice() {
                                gearing(ui, &mut control.to);
                            }
                        },
                    );
                } else if running_on {
                    let_go = ui.small_button("Now").clicked();
                }
                if let_go {
                    clock.reset();
                }

                asked
            },
        )
        .inner;
    if clicked {
        control.out = !control.out;
    }

    // From the state the pass began in, and not from what the click has just
    // asked for. The pane around this is framed and sized before anything in
    // it is drawn -- see [`Dropping`] -- so a rail drawn on the pass the
    // click arrived is a rail drawn in a strip still the width of the
    // reading, with no frame under it and no fill behind it: for one frame
    // the slider hung outside its own panel. What a click asks for is the
    // next pass's to draw, which is where the frame and the width will be
    // waiting for it.
    if out {
        clock_control(ui, clock, turns.geared(control.to));
    }
}

/// Which of the turns on offer the rail covers
///
/// Two words at the far end of the reading, with the rail they are about
/// directly underneath. Only where there are two turns to choose between —
/// see [`Turns::choice`] — a switch between one thing and the same thing
/// being a control that does nothing.
///
/// Not in the settings pane, which is where what the map is drawn like is
/// set. This is about the control beneath it and nothing else: it changes
/// what one drag is worth and says so by changing the marks under the rail.
///
/// Drawn backwards, because the row it stands in runs from the right: `Body`
/// last is `Body` leftmost, so the pair reads narrower first, in the order
/// [`GearedTo::ALL`] holds and the rails themselves run.
fn gearing(ui: &mut Ui, to: &mut GearedTo) {
    greyed(ui, |ui| {
        for offered in GearedTo::ALL.into_iter().rev() {
            ui.selectable_value(to, offered, offered.said())
                .on_hover_text(offered.hint());
        }
    });
}

/// One weak word of the status line, and whether it was clicked
///
/// Every part of the reading answers a click, the date and the span past the
/// present alike: they are two halves of the one moment, and a reader
/// reaching for the number they mean to change should not have to know which
/// half of it the control hangs off. `Now` is the exception, being a control
/// already and one about the same thing.
fn reading(ui: &mut Ui, said: String) -> bool {
    ui.add(
        egui::Label::new(egui::RichText::new(said).weak())
            .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .on_hover_text("Set what moment the system is drawn at")
    .clicked()
}

/// The smallest span the status slider runs the map on by, in seconds
///
/// The near end of a logarithmic rail has to stand at some span rather than
/// at none, since no run of decades reaches zero. A minute: the clock is
/// stepped in whole seconds, and the fastest thing the journal records comes
/// round in hours, so a minute is a hundredth of the quickest turn there is
/// and nothing slower stirs enough to see.
///
/// Zero itself is still the far near end of the rail, egui putting the value
/// exactly at the range's start where the handle is run all the way down.
const SPAN_FLOOR: f64 = 60.;

/// What the status rail is geared to
///
/// One turn of something either way, and which thing settles both how long
/// the rail is and how it is laid out.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Geared {
    /// One turn of the body picked out
    ///
    /// Laid evenly, as that body's own phase slider is: the rail is one orbit
    /// of the one thing being watched, every stretch of it is the same span,
    /// and halfway along is half a turn. Nothing about the span asked for is
    /// lopsided, so nothing about the rail should be.
    Body(f64),
    /// The widest orbit the system has on record
    ///
    /// Laid by decades, because with nothing picked out the spans worth
    /// asking for are not evenly spread: the widest orbit of a system takes a
    /// median eighteen years to come round and its fastest body a few hours,
    /// so an even rail over the whole of that spends its length on spans that
    /// blur every inner body and cannot be nudged by an hour anywhere.
    System(f64),
}

impl Geared {
    /// How long the rail runs, in seconds
    fn turn(self) -> f64 {
        match self {
            Geared::Body(turn) | Geared::System(turn) => turn,
        }
    }

    /// Whether every stretch of the rail is the same span
    fn even(self) -> bool {
        matches!(self, Geared::Body(_))
    }
}

/// A slider over one turn, under the reading
///
/// What the date opens. Geared to the body picked out where there is one, and
/// to the widest orbit the system has otherwise, so its far end is that thing
/// one turn on: watching a planet, the rail is that planet's year, and with
/// nothing picked out it is the span in which the whole system has been
/// through every arrangement it takes. The sliders under the bodies do the
/// same for a body whose panel is open, and this is the one that needs no
/// panel, which is the whole reason it is here.
///
/// No numbers of its own. What it comes to is a moment, and the moment is the
/// line above it -- along with the span it stands past the present, which is
/// what a number on the rail would have said and says it in the units a
/// reader thinks in.
///
/// The span outright rather than a phase, which is the difference between a
/// rail and a body's slider. Dragged to the far end it stands at a turn from
/// now and stays there; dragged back it comes back. Read as a phase it wrapped
/// instead -- a whole turn reads as none of one -- so the far end put the
/// handle back at the near end with the map a turn out, and the next drag
/// measured from the turn after that. Where the turn was the ceiling itself
/// there was nowhere further to go and the reading stuck until `Now`.
///
/// One range, and its far end is [`Clock::CEILING`] where a turn runs past
/// that. The rail's end and the furthest the clock will go have to be the
/// same number or the last stretch of the rail asks for spans the clock
/// answers with the one it stopped at: the handle stands where the pointer
/// put it, the reading stands at the ceiling, and the two disagree for as
/// long as the drag lasts.
///
/// Nothing to drag where no orbit in the system has a period recorded: there
/// is no turn to cover, and a slider over nothing would move the map by
/// nothing however far it was dragged.
///
/// Its width is the strip's, taken as a number rather than as what the line
/// above it left over. That line grows with the reading -- a fifth digit in
/// the year, a longer span beside it -- and egui widens a `Ui` to hold a row
/// too wide for it, so once the line is wider than the strip the room left
/// under it moves with what the rail last set. Which is a control whose scale
/// is a function of its own value: at a span of some ten thousand years the
/// two settled into a cycle, one width setting the span that asks for the
/// other, and the reading flicked between two moments a log step apart under
/// a hand holding still. Measured pixel by pixel along the rail:
/// `11090.83y then 7922.02y`, over and over. Held to
/// `the_reading_holds_still_all_along_the_rail`.
fn clock_control(ui: &mut Ui, clock: &mut Clock, geared: Option<Geared>) {
    let turn = geared.map_or(0., Geared::turn).min(Clock::CEILING);
    // Where the map already stands, as much of it as this rail covers. A
    // body's own slider can have run the offset past a turn of the widest
    // orbit; the rail then reads at its far end rather than wrapping round.
    let mut past = clock.offset().min(turn);
    ui.spacing_mut().slider_width = RAIL_WIDTH - ui.spacing().item_spacing.x;
    // Thinner than the sliders in the pane, and shorter in its row. Those are
    // read one to a line down a column of controls; this is one rail across
    // the top of the map, and egui's own proportions drew it as a grey bar
    // over the sky with a lozenge in it.
    ui.spacing_mut().slider_rail_height = RAIL_HEIGHT;
    ui.spacing_mut().interact_size.y = RAIL_ROOM;
    let moved = ui
        .add_enabled_ui(turn > 0., |ui| {
            let mut rail = egui::Slider::new(&mut past, 0.0..=turn)
                .show_value(false)
                .handle_shape(egui::style::HandleShape::Circle);
            if !geared.is_some_and(Geared::even) {
                rail = rail.logarithmic(true).smallest_positive(SPAN_FLOOR);
            }
            ui.add(rail)
        })
        .inner;
    if moved.changed() {
        clock.offset_at(past);
    }

    if let Some(geared) = geared {
        marks(ui, moved.rect, geared);
    }
}

/// How thick the rail is drawn
const RAIL_HEIGHT: f32 = 4.;

/// How tall a row the rail is given
///
/// The handle is sized off it — egui draws one at a fifth of the row either
/// side of the rail — so this is what settles how big the thing under the
/// pointer is. Enough to hit and no more.
const RAIL_ROOM: f32 = 14.;

/// How far apart the handle's circle stands from the rail's own ends
///
/// Egui shrinks the range the handle travels in by its own radius at either
/// end, so a value's place along the rail is measured in what is left rather
/// than in the whole of it. The same fifth of the row it draws the handle at.
fn handle_radius(rail: egui::Rect) -> f32 {
    rail.height() / 2.5
}

/// A minute, and the units built on it, in seconds
const MINUTE: f64 = 60.;

const HOUR: f64 = 60. * MINUTE;

const DAY: f64 = 24. * HOUR;

const YEAR: f64 = 365.25 * DAY;

/// The spans a logarithmic rail is marked at
///
/// One or two to a decade, at the spans a reader thinks in rather than at
/// round numbers of seconds: an hour, a day, a year. Which of them are drawn
/// is what the rail covers and what fits — see [`marks`] — so this is every
/// mark the map might make, from the rail's own near end at [`SPAN_FLOOR`] up
/// past the ten thousand years a wide pair takes to come round.
const MARKED: [f64; 12] = [
    MINUTE,
    10. * MINUTE,
    HOUR,
    6. * HOUR,
    DAY,
    7. * DAY,
    30. * DAY,
    YEAR,
    10. * YEAR,
    100. * YEAR,
    1_000. * YEAR,
    10_000. * YEAR,
];

/// Say how far along the rail `span` falls, as a fraction of its length
///
/// The same arithmetic egui lays the handle out by, so a mark stands under
/// the place the handle stops at rather than near it: linear over a body's
/// own turn, and over the decades from [`SPAN_FLOOR`] otherwise. Anything at
/// or under the near end is the near end, which is where nothing and a minute
/// both stand on a rail that runs to years.
fn along(span: f64, turn: f64, even: bool) -> f32 {
    if turn <= 0. {
        return 0.;
    }
    if even {
        return (span / turn).clamp(0., 1.) as f32;
    }
    if span <= SPAN_FLOOR {
        return 0.;
    }
    let floor = SPAN_FLOOR.log10();
    let ceiling = turn.log10();
    if ceiling <= floor {
        return 0.;
    }
    (((span.log10() - floor) / (ceiling - floor)) as f32).clamp(0., 1.)
}

/// Write what the rail's places come to, under it
///
/// A rail over one turn of a system runs from a minute to millennia and says
/// nothing about where along it a day is. Named marks are what make a
/// logarithmic rail readable: the handle stands over a word rather than a
/// third of the way along nothing.
///
/// The near end is `now`, that being where the rail's own floor and no span
/// at all both stand, and the far end is however long the turn is. Between
/// them, the spans of [`MARKED`] the turn covers — or the quarters of it,
/// where the rail is a body's own turn laid evenly and decades would mark one
/// end of it.
///
/// Whatever will not fit is left out, left to right: a mark is drawn only
/// where it stands clear of the last one written. The far end is written
/// first for that reason, being the one a reader needs — it says what the
/// rail covers — so a mark that would run into it is the one that goes.
fn marks(ui: &mut Ui, rail: egui::Rect, geared: Geared) {
    let turn = geared.turn().min(Clock::CEILING);
    if turn <= 0. {
        return;
    }

    let even = geared.even();
    let mut wanted = vec![(0_f64, "now".to_owned())];
    if even {
        for quarter in 1..4 {
            let span = turn * quarter as f64 / 4.;
            wanted.push((span, briefly(span, false)));
        }
    } else {
        for span in MARKED.into_iter().filter(|span| *span < turn) {
            wanted.push((span, briefly(span, false)));
        }
    }
    wanted.push((turn, briefly(turn, false)));

    let gap = ui.spacing().item_spacing.x;
    let inset = handle_radius(rail);
    let ends = egui::Rangef::new(rail.left() + inset, rail.right() - inset);
    let written: Vec<(f32, std::sync::Arc<egui::Galley>)> = wanted
        .into_iter()
        .map(|(span, said)| {
            let at = egui::lerp(ends, along(span, turn, even));
            let galley = egui::WidgetText::from(
                egui::RichText::new(said).weak().small(),
            )
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Small,
            );
            (at, galley)
        })
        .collect();

    let row = ui
        .allocate_exact_size(
            egui::vec2(
                rail.width(),
                written.first().map_or(0., |(_, said)| said.size().y),
            ),
            egui::Sense::hover(),
        )
        .0;

    // The far end first, then the rest from the near end up, so that what is
    // dropped where the two meet is a mark in the middle rather than the one
    // saying how far the rail goes.
    let mut taken: Vec<egui::Rangef> = Vec::with_capacity(written.len());
    let order = written.len().saturating_sub(1);
    for index in std::iter::once(order).chain(0..order) {
        let Some((at, said)) = written.get(index) else { continue };
        let across = said.size().x;
        // Centered on the mark, and held inside the row at either end: the
        // near end's word would otherwise hang off the strip by half of
        // itself.
        let left = (at - across / 2.).clamp(row.left(), row.right() - across);
        let stands = egui::Rangef::new(left - gap, left + across + gap);
        if taken.iter().any(|held| held.intersects(stands)) {
            continue;
        }
        taken.push(stands);
        ui.painter().galley(
            egui::pos2(left, row.top()),
            said.clone(),
            egui::Color32::PLACEHOLDER,
        );
    }
}

/// A span in the largest unit it fills, in as few characters as say it
///
/// For the strip, where the reading, the switch and `Now` share one line and
/// a dozen marks share the one under it:
/// `crate::ui::panels::fields::lasting` writes `18.0 Earth years`, which is the
/// right answer in a panel and four marks' worth of room here — and at the
/// ceiling, `+14989.7 Earth years`, which ran clean through the switch.
///
/// `fine` asks for a tenth of the unit, which is what the reading wants and
/// the marks do not: a mark stands at a span chosen to be a whole one, where
/// a reading has to move as the rail is dragged. `+3 h` held for every drag
/// across a stretch of the rail is a reading that looks stuck.
///
/// Years are whole and grouped either way. A tenth of a year is not
/// something a reader is asking about out there, and `14989.7 y` is a length
/// rather than a number; the date beside it is where the moment itself is
/// read.
fn briefly(span: f64, fine: bool) -> String {
    let tenths = usize::from(fine);
    if span < HOUR {
        format!("{:.*} min", tenths, span / MINUTE)
    } else if span < DAY {
        format!("{:.*} h", tenths, span / HOUR)
    } else if span < 60. * DAY {
        format!("{:.*} d", tenths, span / DAY)
    } else if span < 330. * DAY {
        // Months only where a month is the largest unit filled. A year read
        // as `12 mo` is the right number in the wrong unit, and the mark
        // beside it says `10 y`.
        format!("{:.*} mo", tenths, span / (30. * DAY))
    } else {
        format!("{} y", thousands((span / YEAR).round() as u64))
    }
}

/// How far ahead of ours the game's own calendar runs, in years
///
/// The two run together otherwise: an hour out there is an hour here, and the
/// journal stamps its scans in our own time. Which is why the span the clock
/// holds needs no converting at all and only the year it lands in does.
const AHEAD_BY: i32 = 1286;

/// The moment the map is standing at, by the game's calendar
///
/// [`Clock::moment`] in our own, turned once here: the two calendars run
/// together and only the year is 1286 apart.
///
/// In the game's own notation: the day before the month, the month named
/// rather than numbered, the time to the second, and the whole of it in
/// capitals. A reader comparing the map against the panel in front of them is
/// comparing two of the same thing, and a named month cannot be read the
/// American way round by mistake. The date leads, the line being read as a
/// date that carries a time rather than as a clock.
///
/// The year is written out here rather than by `%Y`, which puts a `+` in
/// front of anything past four digits: that is ISO 8601 saying the year is an
/// expanded one, and it reads on the map as a span rather than as a date. Five
/// digits is reachable and not even far-fetched -- the calendar already stands
/// 1286 years on, and a system whose widest orbit is a wide pair's takes
/// millennia to come round, so running the slider to the end of one lands
/// there.
fn drawn_at(clock: &Clock) -> String {
    let drawn = clock.moment();
    let year = drawn.year() + AHEAD_BY;
    let dated = drawn
        .with_year(year)
        // The one day of ours a game year may not hold: a leap day landing
        // 1286 years on in a year without one. Read as the last day of that
        // February, which is the nearest date there is to it.
        .or_else(|| drawn.with_day(28).and_then(|day| day.with_year(year)))
        .unwrap_or(drawn);

    format!("{} {year} {}", dated.format("%d %b"), dated.format("%H:%M:%S"))
        .to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::words;
    use crate::ui::bar::ask_box;

    use crate::ui::testing::{clicking, dragged, placed, spoken_at};

    use crate::ui::{BAR_WIDTH, GEAR_ROOM, MARGIN};
    use chrono::{DateTime, Utc};

    /// A moment out in the galaxy, ours
    fn ours(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the fixture is a moment")
            .with_timezone(&Utc)
    }

    /// A clock standing at `text`, in our own time
    fn standing(text: &str) -> Clock {
        let mut clock = Clock::default();
        clock.follows(ours(text));
        clock
    }

    /// The game's calendar runs 1286 years ahead of ours and otherwise with it
    ///
    /// The journal stamps its scans in our own time, so the span the clock
    /// holds needs no converting and only the year it lands in does.
    #[test]
    fn the_games_calendar_runs_1286_years_ahead() {
        let clock = standing("2014-12-16T13:45:00Z");

        assert_eq!(drawn_at(&clock), "16 DEC 3300 13:45:00");
    }

    /// And the run-on is carried into it
    ///
    /// The moment on screen is the present the map is standing at plus
    /// however far a slider has run it on.
    #[test]
    fn the_moment_shown_carries_how_far_the_map_has_run_on() {
        let mut clock = standing("2015-01-01T00:00:00Z");
        clock.offset_to(365. * 86_400., 1.);

        assert_eq!(
            drawn_at(&clock),
            "01 JAN 3302 00:00:00",
            "a year on from a new year is the next one"
        );
    }

    /// A leap day reads as the last day of its own February
    ///
    /// 1286 years on from one of ours is not always a year with a 29th in it,
    /// and there is no such date to show. The 28th is the nearest there is.
    #[test]
    fn a_leap_day_reads_as_the_last_of_its_february() {
        let clock = standing("2024-02-29T09:00:00Z");

        assert_eq!(drawn_at(&clock), "28 FEB 3310 09:00:00");
    }

    /// A year past four digits is said as a year, without a sign
    ///
    /// Reported from the map: `18 MAR +10284 17:06:50`. `%Y` marks a year
    /// outside the four-digit range as an expanded one the ISO way, and a `+`
    /// in the middle of a date reads as a span. Reachable without trying: the
    /// calendar already stands 1286 years on, and the slider covers one turn
    /// of the system's widest orbit, which for a wide pair is millennia.
    #[test]
    fn a_year_past_four_digits_is_said_without_a_sign() {
        let mut clock = standing("2015-01-01T00:00:00Z");
        clock.offset_to(9000. * 365.25 * 86_400., 1.);

        assert_eq!(drawn_at(&clock), "10 MAR 12301 00:00:00");
    }

    /// The date is said with no system held at all
    ///
    /// The moment is the galaxy's rather than a system's, so there is one to
    /// read out between systems as much as inside one -- and `Now`, the only
    /// way to let go of a run-on, rides that line. Drawn off the held
    /// system's newest scan, the line went out on the way between two systems
    /// and took the way back with it.
    #[test]
    fn the_date_is_said_without_a_system_held() {
        let mut clock = standing("2015-01-01T00:00:00Z");
        let said = words(|ui| {
            dated(
                ui,
                &mut clock,
                Turns::default(),
                &mut ClockControl::default(),
            );
        });

        assert!(said.contains(&"01 JAN 3301 00:00:00".to_owned()), "{said:?}");
    }

    /// Clicking the date opens the slider under it, and clicking it again
    /// puts it away
    ///
    /// The reading is where a user meets the clock, so a reader who wants
    /// another moment reaches for the moment on screen rather than hunting the
    /// pane for a control they have never seen. Drawing the line opens
    /// nothing on its own.
    #[test]
    fn clicking_the_date_opens_the_slider_under_it() {
        let ctx = crate::testing::context();
        let turn = 400. * 86_400.;
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut line = |input, control: &mut ClockControl| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                dated(ui, &mut clock, system_turn(turn), control);
                at = ui.min_rect();
            });
            at
        };

        // Two passes with nothing happening, to place the line.
        let _ = line(egui::RawInput::default(), &mut control);
        let at = line(egui::RawInput::default(), &mut control);
        assert!(!control.out, "the line opened the slider unbidden");

        let date = at.left_center() + egui::vec2(4., 0.);
        line(clicking(date), &mut control);
        assert!(control.out, "a click on the date opened nothing");

        line(clicking(date), &mut control);
        assert!(!control.out, "a second click left the slider out");
    }

    /// Every word painted in `output`
    ///
    /// [`words`] runs a pass of its own, which is no use where what is wanted
    /// is what one pass of several said.
    fn spoken(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => {
                    into.push(text.galley.text().to_owned())
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut found);
        }
        found
    }

    /// The pass that asks for the rail does not draw it
    ///
    /// Reported: opening the strip, the slider looked to be outside its own
    /// panel for a moment. The pane is framed and sized before anything in it
    /// is drawn — see [`Dropping`] — so a rail drawn on the pass the click
    /// arrived is a rail drawn in a strip still the width of the reading,
    /// with no frame under it and no fill behind it.
    ///
    /// The marks are how it shows: they are the only words the rail paints,
    /// so a pass with `now` in it is a pass that drew a rail.
    #[test]
    fn the_pass_that_opens_the_rail_does_not_draw_it() {
        let ctx = crate::testing::context();
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut line = |input, control: &mut ClockControl| {
            let mut at = egui::Rect::NOTHING;
            let output = ctx.run_ui(input, |ui| {
                ui.set_width(STRIP_WIDTH);
                dated(ui, &mut clock, system_turn(400. * DAY), control);
                at = ui.min_rect();
            });
            (at, spoken(&output))
        };

        // Two passes with nothing happening, to place the line.
        let _ = line(egui::RawInput::default(), &mut control);
        let (at, said) = line(egui::RawInput::default(), &mut control);
        assert!(!said.contains(&"now".to_owned()), "a rail unbidden: {said:?}");

        let date = at.left_center() + egui::vec2(4., 0.);
        let (_, asking) = line(clicking(date), &mut control);
        assert!(control.out, "a click on the date opened nothing");
        assert!(
            !asking.contains(&"now".to_owned()),
            "the rail was drawn on the pass that asked for it: {asking:?}"
        );

        // And the next pass has the frame and the width waiting for it.
        let (_, drawn) = line(egui::RawInput::default(), &mut control);
        assert!(drawn.contains(&"now".to_owned()), "{drawn:?}");
    }

    /// And so does clicking the span past the present
    ///
    /// The date and the span are halves of the one moment. A reader who has
    /// run the map on is reading the span, and that is the number they reach
    /// for to change it.
    #[test]
    fn clicking_the_span_opens_the_slider_too() {
        let ctx = crate::testing::context();
        let turn = 400. * 86_400.;
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut line = |offset: f64, input, control: &mut ClockControl| {
            let mut clock_ = Clock::default();
            clock_.offset_at(offset);
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                dated(ui, &mut clock_, system_turn(turn), control);
                at = ui.min_rect();
            });
            clock = clock_;
            at
        };

        // The date alone, to measure how far along the row the span starts.
        let _ = line(0., egui::RawInput::default(), &mut control);
        let dateless = line(0., egui::RawInput::default(), &mut control);

        // And again with the map run on, so the span stands beside it.
        let span = 200. * 86_400.;
        let _ = line(span, egui::RawInput::default(), &mut control);
        let whole = line(span, egui::RawInput::default(), &mut control);
        assert!(
            whole.width() > dateless.width(),
            "the span was not drawn beside the date"
        );

        let on = egui::pos2(dateless.right() + 8., dateless.center().y);
        line(span, clicking(on), &mut control);

        assert!(control.out, "a click on the span opened nothing");
        assert_eq!(clock.offset(), span, "the click moved the map");
    }

    /// A rail geared past the ceiling stops where a date runs out
    ///
    /// Reported as a crash: `DateTime + TimeDelta` overflowed. The ceiling is
    /// the room left between now and the end of chrono's calendar once the
    /// game's 1286 years are added, so the far end of even an absurd rail is
    /// a moment that can still be written. Only a turn longer than that
    /// reaches it, which is a wide pair's and nothing a body has.
    #[test]
    fn a_rail_past_the_ceiling_stops_at_it() {
        let year = 365.25 * 86_400.;
        let ran = slid(Geared::System(400_000. * year), &[(0., 2.)]);

        assert_eq!(ran.offset(), Clock::CEILING);

        let mut clock = standing("2015-01-01T00:00:00Z");
        clock.offset_at(ran.offset());
        assert_eq!(drawn_at(&clock), "19 FEB 253306 00:00:00");
    }

    /// The status slider runs the system on by one turn of its widest orbit
    ///
    /// Which is the whole point of gearing it to that one: a control over a
    /// whole system has to reach every arrangement the system passes through,
    /// and nothing past them. Run end to end it comes to exactly that turn,
    /// however far the drag is carried on past the rail.
    #[test]
    fn the_status_slider_runs_the_system_on_by_its_widest_turn() {
        let turn = 400. * 86_400.;
        let clock = slid(Geared::System(turn), &[(0., 2.)]);

        assert_eq!(
            clock.offset(),
            turn,
            "the slider ran the map on by something other than a turn"
        );
    }

    /// And its near end is decades of span rather than a even share of one
    ///
    /// The spans worth asking for are not evenly spread: a system's widest
    /// orbit takes a median eighteen years and its fastest body a few hours,
    /// so half the rail spent on half of eighteen years is a control that
    /// cannot be nudged by an hour at all. Halfway along a logarithmic rail
    /// stands at the geometric middle instead, which is hours rather than
    /// years.
    #[test]
    fn the_status_slider_is_finer_near_the_present() {
        let turn = 400. * 86_400.;
        let middle = slid(Geared::System(turn), &[(0., 0.5)]).offset();

        assert!(middle > 0., "halfway along the rail moved nothing");
        assert!(
            middle < turn / 100.,
            "halfway along the rail stood at {middle} of {turn}, \
             which is an even share of the turn"
        );
    }

    /// And the far end is a place to come back from
    ///
    /// Reported: dragged to the right end, the rail broke and the reading
    /// stuck at the ceiling until `Now`. It set a phase, and a whole turn
    /// reads as none of one, so the far end put the handle back at the near
    /// end with the map a turn out and the next drag measured from the turn
    /// after that -- which, where the turn was the ceiling, had nowhere to
    /// go. The rail sets the span outright, so a second drag means what it
    /// says wherever the first one left the map.
    #[test]
    fn the_status_slider_comes_back_from_its_far_end() {
        let turn = 400. * 86_400.;
        let back =
            slid(Geared::System(turn), &[(0., 2.), (0.98, 0.5)]).offset();

        assert!(back > 0., "the map came back further than it was dragged");
        assert!(
            back < turn / 100.,
            "a drag back to the middle of the rail left the map at {back} \
             of {turn}"
        );
    }

    /// And nowhere along it does the reading move under a still hand
    ///
    /// Reported: dragging out past some ten thousand years, the time flicked
    /// between two numbers until the drag carried on past it, and again on
    /// the way back down. The rail took its width from what the line above it
    /// left over, and that line grows with the reading -- a fifth digit in
    /// the year, a longer span beside it -- so the width the rail was
    /// measured at moved with the span the rail had last set. One width asked
    /// for the span that asked for the other, frame after frame.
    ///
    /// Walked pixel by pixel, every one of them held for two frames, because
    /// the cycle only appears where the line's width crosses the bar's and no
    /// single place along the rail would have found it.
    #[test]
    fn the_reading_holds_still_all_along_the_rail() {
        let ctx = crate::testing::context();
        let year = 365.25 * 86_400.;
        let mut clock = Clock::default();
        let control = |input, clock: &mut Clock| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                ui.set_width(BAR_WIDTH);
                dated(
                    ui,
                    clock,
                    system_turn(Clock::CEILING),
                    &mut ClockControl { out: true, ..Default::default() },
                );
                at = ui.min_rect();
            });
            at
        };

        let _ = control(egui::RawInput::default(), &mut clock);
        let at = control(egui::RawInput::default(), &mut clock);
        let y = at.bottom() - 8.;
        let start = egui::pos2(at.left() + 4., y);
        control(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton {
                        pos: start,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
                ..Default::default()
            },
            &mut clock,
        );

        let mut moved = Vec::new();
        for step in 0..(at.width() as i32) {
            let on = egui::pos2(start.x + step as f32, y);
            let mut held = Vec::new();
            for _ in 0..2 {
                control(
                    egui::RawInput {
                        events: vec![egui::Event::PointerMoved(on)],
                        ..Default::default()
                    },
                    &mut clock,
                );
                held.push(clock.offset() / year);
            }
            if held[0] != held[1] {
                moved.push(format!(
                    "{step}px: {:.2}y then {:.2}y",
                    held[0], held[1]
                ));
            }
        }

        assert!(moved.is_empty(), "the reading moved on its own: {moved:?}");
    }

    /// Where the strip stood, drawn over a viewport `across` wide
    ///
    /// Several frames, since a strip that has just been opened is measured
    /// from the pass before it: an area is placed at the size it last came
    /// out at.
    fn stripped(across: f32, chrome_right: f32, out: bool) -> egui::Rect {
        let ctx = crate::testing::context();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(across, 800.),
            )),
            ..Default::default()
        };
        let mut clock = Clock::default();
        let mut control = ClockControl { out, ..Default::default() };
        let mut at = egui::Rect::NOTHING;
        for _ in 0..4 {
            let _ = ctx.run_ui(input.clone(), |ui| {
                at = time_strip(
                    ui.ctx(),
                    chrome_right,
                    &mut clock,
                    &mut control,
                    system_turn(400. * DAY),
                );
            });
        }

        at
    }

    /// The reading stands in the middle of the top edge
    #[test]
    fn the_strip_stands_in_the_middle_of_the_top() {
        let wide = stripped(1600., 400., false);

        assert!(
            (wide.center().x - 800.).abs() < 2.,
            "the reading stood at {}, not the middle of 1600",
            wide.center().x
        );
        assert!(wide.top() >= MARGIN, "{wide:?} stood against the top edge");
    }

    /// Shut, it claims no more of the sky than the words take
    ///
    /// It is drawn over the map, and an area is the pointer's wherever it
    /// reaches. Laid out at the width the scrubber wants, a shut strip would
    /// take a band across the top of the viewport for a line of text, and a
    /// wheel turned up there would stop turning the map.
    #[test]
    fn a_shut_strip_takes_only_the_room_the_reading_wants() {
        let shut = stripped(1600., 400., false);

        assert!(
            shut.width() < STRIP_WIDTH / 2.,
            "a reading took {} of {STRIP_WIDTH}",
            shut.width()
        );
    }

    /// A pane in the middle stands where it belongs the first pass it is out
    ///
    /// Reported as opening badly: it appeared, then moved and grew, all
    /// inside a frame or two. A pane is placed before it is drawn, so it has
    /// to be placed by a width it does not have yet; taken off the area's own
    /// rect that is last pass's width, which on the pass a pane opens on is
    /// the width of the reading it has just stopped being — a strip half the
    /// width, centered as though it were still shut, and then a jump.
    ///
    /// So the width the body is about to take is worked out rather than
    /// remembered, and this is what holds it to that: shut for a few passes,
    /// then one pass out, and the strip is where it will still be on the
    /// next.
    #[test]
    fn a_pane_opens_where_it_will_stand() {
        let across = 1600.;
        let ctx = crate::testing::context();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(across, 800.),
            )),
            ..Default::default()
        };
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut stood = |ctx: &Context, control: &mut ClockControl| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input.clone(), |ui| {
                at = time_strip(
                    ui.ctx(),
                    400.,
                    &mut clock,
                    control,
                    system_turn(400. * DAY),
                );
            });
            at
        };

        for _ in 0..4 {
            stood(&ctx, &mut control);
        }

        // The pass it opens on, and the pass after it, which is where the
        // strip settles.
        control.out = true;
        let opened = stood(&ctx, &mut control);
        let settled = stood(&ctx, &mut control);

        assert!(
            (opened.center().x - settled.center().x).abs() < 1.,
            "it opened at {} and settled at {}",
            opened.center().x,
            settled.center().x
        );
        assert!(
            (opened.width() - settled.width()).abs() < 1.,
            "it opened {} wide and settled at {}",
            opened.width(),
            settled.width()
        );
        assert!(
            (settled.center().x - across / 2.).abs() < 2.,
            "and settled off center, at {}",
            settled.center().x
        );

        // And the same on the way back, which is the other half of the same
        // report: the pass it shuts on is placed by what it came to the last
        // time it stood shut rather than by the open width it has just
        // stopped being.
        control.out = false;
        let shut = stood(&ctx, &mut control);
        let resting = stood(&ctx, &mut control);

        assert!(
            (shut.center().x - resting.center().x).abs() < 1.,
            "it shut at {} and came to rest at {}",
            shut.center().x,
            resting.center().x
        );
    }

    /// Out, it is at least as wide as the rail it holds
    ///
    /// The rail is scaled to [`RAIL_WIDTH`] whatever room it is given — see
    /// [`clock_control`] for why it is a number — so a strip narrower than
    /// that is a rail painted out past its own frame.
    #[test]
    fn an_open_strip_holds_its_rail() {
        let out = stripped(1600., 400., true);

        assert!(
            out.width() >= RAIL_WIDTH,
            "{} against a rail of {RAIL_WIDTH}",
            out.width()
        );
        // And is the wider of the two states by some way, the reading alone
        // being a fraction of it.
        assert!(out.width() > stripped(1600., 400., false).width() * 2.);
    }

    /// Where the reading stood, drawn over a viewport `across` wide
    ///
    /// Several passes, since an area paints nothing at all on the first of
    /// them and is placed by what it last came to.
    fn read_at(
        ctx: &Context,
        across: f32,
        chrome_right: f32,
        clock: &mut Clock,
        control: &mut ClockControl,
    ) -> egui::Rect {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(across, 800.),
            )),
            ..Default::default()
        };
        let said = drawn_at(clock);
        let mut found = None;
        for _ in 0..3 {
            let output = ctx.run_ui(input.clone(), |ui| {
                time_strip(
                    ui.ctx(),
                    chrome_right,
                    clock,
                    control,
                    system_turn(400. * DAY),
                );
            });
            found = spoken_at(&output, &said);
        }

        found.unwrap_or_else(|| panic!("the date was not drawn: {said}"))
    }

    /// Up against the bar, the reading holds still rather than sliding into it
    ///
    /// The pane is centered on the viewport, so a body wider than the head
    /// grows to both sides of it and the reading slides left as the rail comes
    /// out. There is room for that on a wide window and none on a narrow one,
    /// where what it slides into is the bar: the rail arrived with its near
    /// end against the search box and the date it was opened from had moved.
    ///
    /// So a pane that cannot be centered clear of the bar stands where its
    /// head stood instead, and grows out to the right.
    #[test]
    fn a_strip_against_the_bar_does_not_slide_into_it() {
        let chrome_right = MARGIN + GEAR_ROOM + MARGIN + BAR_WIDTH;
        // Room for the reading in the middle, and none for the rail there.
        let across = chrome_right * 2. + STRIP_WIDTH / 2.;
        let ctx = crate::testing::context();
        let mut clock = Clock::default();
        let mut control = ClockControl::default();

        let shut =
            read_at(&ctx, across, chrome_right, &mut clock, &mut control);
        assert!(
            (shut.center().x - across / 2.).abs() < 4.,
            "the reading stood at {} of {across} shut",
            shut.center().x
        );

        control.out = true;
        let out = read_at(&ctx, across, chrome_right, &mut clock, &mut control);

        assert!(
            (out.left() - shut.left()).abs() < 1.,
            "the reading was at {} and opening took it to {}",
            shut.left(),
            out.left()
        );
        assert!(
            out.left() > chrome_right + MARGIN,
            "and it stood against the bar, at {}",
            out.left()
        );
    }

    /// The reading stands level with the search box
    ///
    /// The three things across the top of the map are one row: the gear, the
    /// bar's box and the clock's reading. The gear is hung on the box's own
    /// middle, so it follows wherever the box goes; the reading is in a pane
    /// of its own and had to be put level by hand. It sat a few points high,
    /// a field being padded inside where a line of text is not.
    ///
    /// Drawn as [`chrome`] draws them: two panes at the same top, in the same
    /// frame, each holding its own first row.
    #[test]
    fn the_reading_stands_level_with_the_box() {
        let ctx = crate::testing::context();
        let mut clock = Clock::default();
        let mut typed = None;
        let mut box_at = egui::Rect::NOTHING;
        let mut painted = None;
        // Several passes: an area is placed at the size it last came out at,
        // and the first of them paints nothing at all.
        for _ in 0..4 {
            painted = Some(ctx.run_ui(egui::RawInput::default(), |ui| {
                let ctx = ui.ctx().clone();
                Dropping {
                    id: "a-bar",
                    standing: Standing::At(egui::pos2(MARGIN, MARGIN)),
                    out: false,
                    width: BAR_WIDTH,
                    holds_width: true,
                }
                .show(&ctx, |ui| {
                    box_at =
                        ask_box(ui, &mut typed, "Search", false, false).0.rect;
                });
                Dropping {
                    id: "a-strip",
                    standing: Standing::At(egui::pos2(600., MARGIN)),
                    out: false,
                    width: STRIP_WIDTH,
                    holds_width: false,
                }
                .show(&ctx, |ui| {
                    dated(
                        ui,
                        &mut clock,
                        Turns::default(),
                        &mut ClockControl::default(),
                    );
                });
            }));
        }

        let output = painted.expect("a pass was drawn");
        let said = drawn_at(&clock);
        let reading = spoken_at(&output, &said)
            .unwrap_or_else(|| panic!("the date was not drawn: {said}"));

        assert!(
            (reading.center().y - box_at.center().y).abs() < 1.,
            "the reading stood at {} and the box at {}",
            reading.center().y,
            box_at.center().y
        );
    }

    /// The widest line the strip can hold does not run into itself
    ///
    /// Reported: run all the way out, the reading ran clean through the
    /// switch. The line is a reading laid out from the left and controls laid
    /// out from the right, and egui does not stop the two meeting in the
    /// middle -- it widens the `Ui` and the words overlap.
    ///
    /// The widest of everything at once: the clock at its ceiling, which is
    /// the longest date and the longest span the map can stand at, and both
    /// turns on offer so that the switch is drawn beside `Now`.
    #[test]
    fn the_widest_reading_does_not_run_into_the_switch() {
        let mut clock = Clock::default();
        clock.offset_at(Clock::CEILING);
        let mut control = ClockControl { out: true, to: GearedTo::System };
        let said = placed(|ui| {
            ui.set_width(STRIP_WIDTH);
            dated(
                ui,
                &mut clock,
                both_turns(12. * YEAR, Clock::CEILING),
                &mut control,
            );
        });

        let at = |word: &str| {
            said.iter()
                .find(|(text, _)| text == word)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{word} was not drawn: {said:?}"))
        };
        // The span past the present is the last of the reading, and the
        // switch is the first of the controls at the other end.
        let span = at(&format!("+{}", briefly(Clock::CEILING, true)));
        let switch = at("Body");

        assert!(
            span.right() < switch.left(),
            "the reading reached {} and the switch began at {}",
            span.right(),
            switch.left()
        );
    }

    /// And on a narrow window it gives way to the bar
    ///
    /// The two are drawn from opposite rules — the bar from the left edge,
    /// the strip from the middle — so on a window narrow enough they meet.
    /// A reading standing over the field being typed into is worse than a
    /// reading standing off center.
    #[test]
    fn the_strip_gives_way_to_the_bar_on_a_narrow_window() {
        let chrome_right = MARGIN + GEAR_ROOM + MARGIN + BAR_WIDTH;
        let narrow = stripped(chrome_right + STRIP_WIDTH, chrome_right, true);

        assert!(
            narrow.left() >= chrome_right + MARGIN,
            "the strip stood at {} over a bar reaching {chrome_right}",
            narrow.left()
        );
    }

    /// The spans written under a rail geared to `turn`
    ///
    /// The whole of what the control letters: the rail itself shows no value,
    /// so every word painted here is a mark.
    fn marked(turn: f64) -> Vec<String> {
        let mut clock = Clock::default();
        words(|ui| {
            ui.set_width(STRIP_WIDTH);
            clock_control(ui, &mut clock, Some(Geared::System(turn)));
        })
    }

    /// The rail says where both of its ends are
    ///
    /// Which is what a logarithmic rail cannot say for itself: it runs from a
    /// minute to millennia and reads as a bare grey bar. The near end is the
    /// present and the far end is what the rail covers, and a reader wanting
    /// to know how far a drag will carry the map is asking about the second.
    #[test]
    fn the_rail_says_where_its_ends_are() {
        let said = marked(18. * YEAR);

        assert!(said.contains(&"now".to_owned()), "{said:?}");
        assert!(said.contains(&"18 y".to_owned()), "{said:?}");
        assert!(said.len() > 3, "nothing between the ends: {said:?}");
    }

    /// A mark stands where the handle stops, not near it
    ///
    /// The marks are laid out by [`along`] and the handle by egui, off the
    /// same range and the same logarithm. Said twice, so it is worth holding
    /// the two together: a rail whose words sit a tenth of the way off
    /// wherever the handle lands is worse than one with no words at all.
    #[test]
    fn a_mark_stands_where_the_handle_stops() {
        let turn = 400. * DAY;
        for asked in [0.25_f32, 0.5, 0.75] {
            let stood = slid(Geared::System(turn), &[(0., asked)]);
            let mark = along(stood.offset(), turn, false);

            assert!(
                (mark - asked).abs() < 0.03,
                "a drag to {asked} of the rail marked at {mark}"
            );
        }
    }

    /// A crowded rail drops marks rather than stacking them
    ///
    /// Every span [`MARKED`] holds falls inside ten thousand years, which is
    /// a dozen words in the width of the strip. What goes is a mark in the
    /// middle; the two ends stay, being what the rail is read by.
    #[test]
    fn a_crowded_rail_drops_marks_rather_than_stacking_them() {
        let said = marked(10_000. * YEAR);

        assert!(said.len() < MARKED.len(), "{said:?}");
        assert!(said.contains(&"now".to_owned()), "{said:?}");
        assert!(said.contains(&"10,000 y".to_owned()), "{said:?}");
        let mut once = said.clone();
        once.sort();
        once.dedup();
        assert_eq!(
            once.len(),
            said.len(),
            "a mark was written twice: {said:?}"
        );
    }

    /// Hiding the clock puts the map back to the present
    ///
    /// The reading is the only place a run-on is shown and the only way back
    /// from one, so a hidden strip over a map standing three hours on is a map
    /// drawing a moment nobody can see or undo.
    #[test]
    fn hiding_the_clock_puts_the_map_back_to_the_present() {
        let mut clock = Clock::default();
        clock.offset_at(3. * HOUR);
        let mut control = ClockControl { out: true, ..Default::default() };

        hidden(&mut clock, &mut control);

        assert_eq!(clock.offset(), 0.);
        assert!(!control.out, "the scrubber was left out over nothing");
    }

    /// A system's turn on offer and no body's
    fn system_turn(turn: f64) -> Turns {
        Turns { body: None, system: Some(turn) }
    }

    /// Both turns on offer, the body's and the system's
    fn both_turns(body: f64, system: f64) -> Turns {
        Turns { body: Some(body), system: Some(system) }
    }

    /// The strip asks which turn the rail is to cover, and answers it
    ///
    /// A body's own year and the span its whole system takes to come round
    /// are two questions, and which one a reader wants is not something the
    /// map can work out from what they clicked: the body picked out says they
    /// are looking at it, not what span they mean to drag over.
    #[test]
    fn the_rail_covers_whichever_turn_was_asked_for() {
        let turns = both_turns(11.9 * YEAR, 14_990. * YEAR);

        assert_eq!(
            turns.geared(GearedTo::Body),
            Some(Geared::Body(11.9 * YEAR))
        );
        assert_eq!(
            turns.geared(GearedTo::System),
            Some(Geared::System(14_990. * YEAR))
        );
    }

    /// And falls back to whichever turn there is
    ///
    /// The choice is between two spans to read. Whether the map can be run on
    /// at all is a different question, and a reader who last asked for a
    /// body's turn should not lose the rail by flying out of the system.
    #[test]
    fn a_turn_that_is_not_on_record_gives_way_to_the_one_that_is() {
        let system = system_turn(400. * DAY);

        assert_eq!(
            system.geared(GearedTo::Body),
            Some(Geared::System(400. * DAY)),
            "a body's turn was asked for where there is none"
        );
        assert_eq!(Turns::default().geared(GearedTo::System), None);
    }

    /// The switch stands only where there are two turns to choose between
    #[test]
    fn the_switch_is_offered_over_two_turns_and_not_one() {
        assert!(both_turns(11.9 * YEAR, 14_990. * YEAR).choice());
        assert!(!system_turn(400. * DAY).choice());
        assert!(!Turns::default().choice());
    }

    /// What the strip says, geared to `turns` and asking `to`
    fn strip_said(turns: Turns, to: GearedTo) -> Vec<String> {
        let mut clock = Clock::default();
        let mut control = ClockControl { out: true, to };
        words(|ui| {
            ui.set_width(STRIP_WIDTH);
            dated(ui, &mut clock, turns, &mut control);
        })
    }

    /// Both turns are named in the strip, and the marks follow the choice
    ///
    /// Which is what says the switch did anything: the rail is a grey bar
    /// either way, and what changes under it is how far one drag carries the
    /// map. Geared to a body of a dozen years the far mark is that dozen;
    /// geared to the system it is the thousands the widest orbit takes.
    #[test]
    fn the_marks_follow_whichever_turn_was_asked_for() {
        let turns = both_turns(12. * YEAR, 14_990. * YEAR);

        let body = strip_said(turns, GearedTo::Body);
        assert!(body.contains(&"Body".to_owned()), "{body:?}");
        assert!(body.contains(&"System".to_owned()), "{body:?}");
        assert!(body.contains(&"12 y".to_owned()), "{body:?}");

        let system = strip_said(turns, GearedTo::System);
        assert!(system.contains(&"14,990 y".to_owned()), "{system:?}");
        assert!(
            !system.contains(&"12 y".to_owned()),
            "the body's turn was still marked: {system:?}"
        );
    }

    /// And the switch is not drawn where there is nothing to switch to
    #[test]
    fn one_turn_is_read_without_a_switch() {
        let said = strip_said(system_turn(400. * DAY), GearedTo::System);

        assert!(!said.contains(&"Body".to_owned()), "{said:?}");
        assert!(!said.contains(&"System".to_owned()), "{said:?}");
    }

    /// Geared to a body, the rail is that body's own turn laid evenly
    ///
    /// The span asked for while a body is being watched is one orbit of it,
    /// and nothing about an orbit is lopsided: halfway along the rail is half
    /// a turn, as it is on that body's own phase slider. Which is the whole
    /// difference from the rail over a system, where the spans run from hours
    /// to millennia and only decades of them fit on one rail.
    #[test]
    fn a_rail_geared_to_a_body_is_laid_evenly() {
        let turn = 400. * 86_400.;
        let middle = slid(Geared::Body(turn), &[(0., 0.5)]).offset();

        assert!(
            (middle - turn / 2.).abs() < turn / 50.,
            "halfway along stood at {middle} of {turn}"
        );
        // And its far end is still that one turn, as the system's rail's is.
        assert_eq!(slid(Geared::Body(turn), &[(0., 2.)]).offset(), turn);
    }

    /// And a drag held out past the far end reads as one moment, not two
    ///
    /// Reported as a flicker at the handoff: the rail's far end and the
    /// furthest the clock would go were different numbers, so the stretch
    /// between them asked for spans the clock answered with the one it
    /// stopped at. The handle stood where the pointer put it, the reading
    /// stood at the ceiling, and the drag flicked between the two of them
    /// frame after frame. One range now, ending exactly where the clock does.
    #[test]
    fn a_drag_held_past_the_far_end_stands_at_one_moment() {
        let ctx = crate::testing::context();
        let year = 365.25 * 86_400.;
        // A pair whose own turn runs well past the ceiling, so the rail is
        // the ceiling's rather than the turn's.
        let turn = 400_000. * year;
        let mut clock = Clock::default();
        let control = |input, clock: &mut Clock| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                clock_control(ui, clock, Some(Geared::System(turn)));
                at = ui.min_rect();
            });
            at
        };

        let _ = control(egui::RawInput::default(), &mut clock);
        let at = control(egui::RawInput::default(), &mut clock);
        for input in dragged(at.left_center(), at.width() * 3.) {
            control(input, &mut clock);
        }

        // Held there, hand still, while the frames go by.
        let mut seen = Vec::new();
        for _ in 0..8 {
            control(egui::RawInput::default(), &mut clock);
            seen.push(clock.offset());
        }

        assert!(
            seen.iter().all(|offset| *offset == Clock::CEILING),
            "the reading moved under a still hand: {seen:?}"
        );
    }

    /// The clock after the status slider is dragged from `from` to `to` of the
    /// rail's own width, gesture after gesture
    ///
    /// Past one is carried off the far end, which is where a drag that means
    /// the whole turn ends up.
    fn slid(geared: Geared, gestures: &[(f32, f32)]) -> Clock {
        let ctx = crate::testing::context();
        let mut clock = Clock::default();
        let mut control = |input| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                clock_control(ui, &mut clock, Some(geared));
                at = ui.min_rect();
            });
            at
        };

        let _ = control(egui::RawInput::default());
        let at = control(egui::RawInput::default());

        for &(from, to) in gestures {
            let rail = egui::pos2(at.left() + at.width() * from, at.center().y);
            for input in dragged(rail, at.width() * (to - from)) {
                control(input);
            }
        }

        clock
    }
}
