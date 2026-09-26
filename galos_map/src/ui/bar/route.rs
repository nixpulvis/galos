//! The route form: which systems a trip runs through, in what order, and for
//! what jump range

use crate::map::route::frontier::Frontiers;
use crate::map::search::{Plot, Search};
use crate::map::selection::Selection;
use crate::ui::bar::tuning::{approximating, planned, searching_says, trading};
use crate::ui::bar::{BarFields, RANGE_WANTED, ask_box};
use crate::ui::text::typed;
use crate::ui::widgets::{check, entered};
use crate::ui::{FIELD_GAP, SPINNER, STOP};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Response, Ui};
use galos_route::graph::{Drive, Routing, Tuning};
use galos_route::tour::Shape;

/// The jump range asked for, or what is wrong with what was asked
fn jump_range(asked: &str) -> Result<f64, &'static str> {
    match asked.trim().parse::<f64>() {
        Ok(range) if range > 0. => Ok(range),
        Ok(_) => Err("Jump range must be more than nothing"),
        Err(_) => Err("Jump range must be a number of light years"),
    }
}

/// What the map needs before it can plot at all, or what is missing
///
/// Two preconditions, and the second is why this stands beside
/// [`jump_range`] rather than inside it. A range is what the user typed; a
/// supercharge table is what the index published, and a route for a drive
/// that can take a jet cone is a question the map cannot answer without one.
///
/// It used to answer anyway. The router read an absent table as a galaxy
/// where nobody has a jet cone and handed back the unaided route — under the
/// supercharged drive's name, since a route carries what it was plotted with
/// and its panel says so. So the map claimed to have plotted something it had
/// not, and the only tell was a jump count that looked high. Said instead,
/// with the command that fixes it.
///
/// An empty table is not this. A published table with nothing in it is an
/// answer — there is nowhere to supercharge — and the unaided route is the
/// right one.
fn plotting(
    asked: &str,
    drive: Drive,
    boosts: &galos_route::Boosts,
) -> Result<f64, &'static str> {
    let range = jump_range(asked)?;
    if drive.named().is_some() && !boosts.published() {
        return Err("No supercharge table in the index. Rebuild it with \
             `galos ingest --from database --index DIR --only boosts`, or \
             let the ingest that writes it publish once more, or plot \
             unaided.");
    }
    Ok(range)
}

/// The systems a route runs through, or why it has nowhere to run
///
/// What is picked out on the map, in the order it was picked, rather than
/// names typed into fields of the form. Picking a system out is already how
/// the user says which one they mean, to the panel that describes it and to
/// the filter built from it, so a route with fields of its own would be
/// asking twice about systems the map is holding for them.
///
/// Two of them at the least. A longer set is a route through every system in
/// it, leg by leg in the order it was picked: the first is where the flying
/// starts, and each after it is where the next leg is plotted to.
pub(super) fn stops_of(
    selection: &Selection,
) -> Result<Vec<&str>, &'static str> {
    // The systems alone. A route runs between places, and a body picked out
    // beside them is a thing inside one rather than a stop to plot to.
    let stops: Vec<&str> =
        selection.systems().map(|system| system.name()).collect();
    match stops.len() {
        0 => Err("Pick two or more systems"),
        1 => Err("Pick one more system"),
        _ => Ok(stops),
    }
}

/// How wide the systems picked out stand, in light years
///
/// The distance between the two that stand furthest apart, which is the
/// widest the set is by any reading: no two are further, and no one system
/// lies outside a sphere that wide.
///
/// Not `spanned`'s reach, which is what the camera stands back by and what
/// a system's shell is drawn to over the orbits inside it. That is measured
/// from the middle of the *box* the systems fill, and the box's middle moves
/// when the set changes -- so dropping a system can leave the rest further
/// from the new middle than any of them were from the old one, and the figure
/// would climb as the set shrank. Measured over pairs it cannot: taking a
/// system away takes its pairs with it and leaves the rest as they were.
///
/// Every pair, which is one comparison for each. A hand-picked set runs to
/// tens rather than thousands, and the alternative is a hull.
///
/// This rather than the legs added up. A route has not been asked for yet, so
/// how far one would run is not knowable: it turns on the order the stops are
/// reached in, and for the cheapest order that turns on a jump range which
/// may not have been typed. How wide the set stands needs none of it, and is
/// the question a set of destinations raises.
///
/// Nothing where fewer than two are picked out: no pair, nothing to be wide.
fn across(selection: &Selection) -> Option<f64> {
    let places: Vec<DVec3> =
        selection.systems().map(|system| system.position()).collect();

    places
        .iter()
        .enumerate()
        .flat_map(|(index, from)| {
            places[index + 1..].iter().map(move |to| from.distance(*to))
        })
        .max_by(f64::total_cmp)
}

/// The order the trip will be flown in, as indices into what is picked out
///
/// The order they were picked, or the cheapest order to reach them all in
/// where that was asked for and a range is in hand to cost a leg with —
/// which is what `cheapest` carries. The ordering itself is
/// [`galos_route::tour`]'s, and `shape` is what it is told about
/// the trip: which end is held, and whether the leg home is costed with the
/// rest.
///
/// One place decides it, so what the form says the trip comes to and what the
/// trip is actually asked for cannot disagree.
fn flown_order(
    selection: &Selection,
    shape: Shape,
    cheapest: Option<f64>,
) -> Vec<usize> {
    let places: Vec<DVec3> =
        selection.systems().map(|system| system.position()).collect();
    match cheapest {
        Some(range) => galos_route::tour::ordered(&places, range, shape),
        None => (0..places.len()).collect(),
    }
}

/// The stops as the trip is to be flown, named
///
/// The order they were picked, or the cheapest order to reach them all in
/// where that was asked for and a range is in hand to cost a leg with. The
/// ordering is [`galos_route::tour`]'s; this is only the naming.
///
/// A ring names its first stop twice, at both ends. The stops are what the
/// legs are cut from — a leg to each name from the one before it — so the
/// leg home is a leg like any other: asked for, walked, drawn and costed.
/// Nothing else about a trip has to know a ring from a line.
///
/// Handed back as names rather than as an order, since names are what a trip
/// is asked for with and what the legs are keyed by.
fn asked_in_order(
    stops: &[&str],
    selection: &Selection,
    shape: Shape,
    cheapest: Option<f64>,
) -> Vec<String> {
    // The systems `stops_of` named, in the same order, so an index into one
    // is an index into the other.
    let mut named: Vec<String> = flown_order(selection, shape, cheapest)
        .into_iter()
        .filter_map(|at| stops.get(at))
        .map(|stop| stop.to_string())
        .collect();

    if shape.loops()
        && let Some(home) = named.first().cloned()
    {
        named.push(home);
    }
    named
}

/// How many legs a trip through `stops` is flown in
///
/// The gaps between the stops, which is one fewer than there are of them —
/// and one apiece for a ring, which has the leg home as well.
fn legs_flown(stops: usize, shape: Shape) -> usize {
    match shape.loops() {
        true => stops,
        false => stops.saturating_sub(1),
    }
}

/// What shape a trip through `stops` is asked for in
///
/// The form holds two flags and they come to one shape, which is what
/// [`galos_route::tour`] is told and what says how many legs the
/// trip is. One reading of them, so the count the form says, the order the
/// stops go out in, and the legs actually plotted cannot disagree.
///
/// A ring only where there is a trip to close. Two stops flown out and back
/// is the one leg twice over, drawn on top of itself, so [`route_body`] does
/// not offer the control — and a flag left standing from a wider set is not
/// obeyed here either, a control nobody can see being no way to let go of
/// one.
///
/// A ring holds its start whatever the other flag says: every way round one
/// costs the same, so a free start is not a choice about cost, and the form
/// puts that control away while a loop is asked for. See [`Shape`].
fn shape_of(fields: &BarFields, stops: usize) -> Shape {
    match (fields.looping && stops > 2, fields.any_start) {
        (true, _) => Shape::Loop,
        (false, true) => Shape::Anywhere,
        (false, false) => Shape::FromFirst,
    }
}

/// What the form says of the systems picked out, before a route is asked for
///
/// A pair is so far apart, which is the whole of what there is to say about
/// two: a route between them runs from the one to the other however it gets
/// there.
///
/// More than two is said as how many legs it will be and how wide they
/// stand — [`legs_flown`], so a ring counts the leg home among them. Not how
/// far the route will run: that turns on the order they are reached in, and
/// the order may turn on a range not yet typed. How much sky they cover is
/// knowable before anything is walked, and is what a set of destinations
/// raises.
fn apart_said(away: f64, legs: usize) -> String {
    match legs {
        0 | 1 => format!("{away:.1} Ly apart"),
        legs => format!("{legs} legs, {away:.1} Ly across"),
    }
}

/// What the route asks, under the box that names its stops
///
/// The bar's one box is the search box in this mode as in that one — a route
/// runs between systems, and systems are found by name — so what it found
/// stands above this and a click on a line adds a stop. See [`ask_bar`](crate::ui::bar::ask_bar) and
/// [`Picking`](crate::ui::bar::search::Picking).
///
/// Which leaves the range, which is asked for here: it is one of the route's
/// settings rather than the question the bar is putting, and it stands with
/// them, above the two that say how the route is to be worked out. It led the
/// form to begin with, in the bar's own box, which cost the mode the box:
/// there was nowhere left to name a system, and the stops were whatever the
/// map already had picked out.
///
/// Answers the range's field, which the caller needs for the same reason it
/// needs the box's: a caret left in either takes the keys the map flies with.
/// See [`Asked::boxes`](crate::ui::bar::Asked::boxes).
///
/// Which systems the route runs through is [`stops_of`]'s to settle, off what
/// is picked out — by a click on the map, on a row, or on a name the box
/// found.
///
/// How it is getting on is said between the settings and the button, where
/// what it is about is on either side of it.
#[allow(clippy::too_many_arguments)]
pub(super) fn route_body(
    ui: &mut Ui,
    search: &mut BarFields,
    selection: &Selection,
    searched: &mut MessageWriter<Search>,
    plot: &mut Plot,
    how: &mut Routing,
    drive: &mut Drive,
    tune: &mut Tuning,
    boosts: &galos_route::Boosts,
    searching: &Frontiers,
) -> Response {
    // Which systems it runs through is not said here. They are the rows in
    // the state bar below, named there and in that order, and a form that
    // spelled them out again would say the same thing twice -- at six stops,
    // in a line of names longer than the bar is wide. What is missing is
    // said, though: a route wants two, and the box above this is where a
    // second one is found.
    let stops = stops_of(selection);
    if let Err(why) = &stops {
        // Weakly. Nothing has gone wrong: the user is part way through
        // asking, and a form in red before it has been filled in is a form
        // scolding whoever fills it in.
        ui.label(egui::RichText::new(*why).weak());
    }
    // What shape the trip is flown in: which end is held, and whether it
    // comes home. Read here rather than beside the controls that set it, so
    // that what the form says the trip comes to, what the ordering is asked
    // for, and what is actually plotted are the one answer.
    let shape = shape_of(search, stops.as_ref().map_or(0, Vec::len));

    // What the map can say about them before a route is asked for: how many
    // legs it will be, and how wide they stand. Nothing about the order they
    // will be reached in, which is what the range settles and what nothing
    // here waits on.
    if let (Ok(stops), Some(away)) = (&stops, across(selection)) {
        let legs = legs_flown(stops.len(), shape);
        ui.label(egui::RichText::new(apart_said(away, legs)).weak());
    }
    ui.add_space(FIELD_GAP);

    // How far the ship jumps unaided, which is the one number a route cannot
    // be worked out without. Nothing stands under it as an answer, so
    // clearing it takes the range and nothing else.
    let (box_, emptied) =
        ask_box(ui, &mut search.route_range, RANGE_WANTED, false, false);
    if emptied {
        search.route_range = None;
    }
    ui.add_space(FIELD_GAP);

    // What the route is weighed by, which is the first question: the trade
    // between the fewest jumps and the least fuel, and the tie-break the
    // one end of it has. Shown in light years off the range typed above,
    // that being the number a reader is holding.
    let jump = search
        .route_range
        .as_deref()
        .and_then(|range| range.parse::<f64>().ok());
    trading(ui, how, jump);

    // Then what it is allowed to trade for the wait, which is a question
    // about the *search* and not about the route — so it stands under
    // what it qualifies rather than over it. Two rails and each other's
    // opposite, and the proven ask is the **top stop of whichever one
    // applies**: `Within` at optimal where the percent bites, and
    // `Expand nearest` at `all` where it does not.
    //
    // There was an `Optimal` tick over them until the top of the rail
    // could say it. It was two controls for one number: ticking it hid
    // the rails, unticking it had to remember where they had been, and
    // the same state was reachable two ways. See
    // [`Routing::approximates`], which is now the one place the question
    // is answered.
    approximating(ui, how);
    ui.add_space(FIELD_GAP);
    // Whether a jet cone counts, and what it is worth. A neutron star
    // supercharges a drive for one jump — four times the range, six off the
    // drive built for it — so a route that may use one runs through the
    // neutron stars on the way rather than in the ship's own reach. The range
    // typed above stays what the ship does unaided; this is what a boost
    // multiplies it by. See `Drive`.
    egui::ComboBox::from_label("Supercharging")
        .selected_text(match *drive {
            Drive::Unaided => "None",
            Drive::Standard => "Standard (x4 / x1.5)",
            Drive::Optimised => "SCO Mk II (x6 / x3)",
        })
        .show_ui(ui, |ui| {
            // The multiples are in the names, so a hint says what they are
            // multiples of rather than saying them twice.
            for (fitted, said, hint) in [
                (
                    Drive::Unaided,
                    "None",
                    "No boosts: every jump is the ship's range",
                ),
                (
                    Drive::Standard,
                    "Standard (x4 / x1.5)",
                    "Boost off neutron stars and white dwarfs",
                ),
                (
                    Drive::Optimised,
                    "SCO Mk II (x6 / x3)",
                    "Bigger boosts off the same stars",
                ),
            ] {
                ui.selectable_value(&mut *drive, fitted, said)
                    .on_hover_text(hint);
            }
        });

    // And how a long supercharged route is planned, which is a question
    // about the *method* rather than about the answer: the two above say
    // what the route has to be, and these say how the coarse plan over the
    // boost stars goes about finding one. See [`planned`].
    if drive.named().is_some() {
        planned(ui, *how, tune);
    }

    // Return in the range asks for the route, as pressing the button does. It
    // is the last thing a route waits on, and a form with one thing left to
    // do should not have to be reached for.
    let submitted = entered(&box_, ui);
    // What came back of the last route asked for answers the range as it was
    // then, so it goes as soon as it is not. Work still under way is not an
    // answer to anything yet, and stays.
    if box_.changed() && matches!(*plot, Plot::Failed(_)) {
        *plot = Plot::Nothing;
    }

    // How the last route asked for is getting on. Only ever a route that
    // was asked for: a field being typed into is not an attempt at
    // anything, and a form that answers back before it has been submitted
    // is a form scolding whoever fills it in.
    if let Plot::Failed(trouble) = &*plot {
        ui.add_space(FIELD_GAP);
        ui.colored_label(egui::Color32::LIGHT_RED, trouble);
    }

    // What shape the trip takes. Only where there is a trip to shape: two
    // stops have one order and one leg between them either way round, and
    // three or more picked out are as likely to be a set of destinations as
    // an itinerary.
    if stops.as_ref().is_ok_and(|stops| stops.len() > 2) {
        check(
            ui,
            &mut search.looping,
            "Loop",
            "Fly home to the first stop at the end",
        );
        check(
            ui,
            &mut search.tour,
            "Cheapest order",
            "Reorder the stops to fly the least",
        );
        // Only under the box it qualifies, and not under a ring: where the
        // order is the user's own there is nothing to hold the start
        // against, and a ring has no free end to hold — every way round one
        // costs the same, so where it is entered is not a choice about cost.
        if search.tour && !search.looping {
            ui.indent("start", |ui| {
                let mut from_first = !search.any_start;
                if check(
                    ui,
                    &mut from_first,
                    "Start w/ First Selected System",
                    "Keep the first stop as the start",
                )
                .changed()
                {
                    search.any_start = !from_first;
                }
            });
        }
    }

    ui.add_space(FIELD_GAP);
    // The two things a route is made of: which systems it runs through, and
    // what it may be flown in. The button is dead until both are in hand,
    // since a plot missing one of them is nothing to ask the router about.
    let asked = stops.ok().zip(typed(&search.route_range));
    // Egui lays a button's contents out as atoms, and a custom atom is a
    // slot of a given size that hands its rect back to be painted into. So
    // the spinner takes a place in the row beside the label rather than
    // being painted over the top of it, and asks for no room at all on a
    // button that has nothing to say.
    let slot = ui.id().with("plotting");
    let mut atoms = egui::Atoms::new("Plot Route");
    if *plot == Plot::Working {
        let turning = ui.text_style_height(&egui::TextStyle::Button) * SPINNER;
        atoms.push_left(egui::Atom::custom(slot, egui::Vec2::splat(turning)));
    }
    // The plot button and, while something is running, a stop beside it.
    // Stopping used to be the plot button's second meaning, which was wrong
    // twice: a control whose meaning depends on invisible state cannot be
    // read before it is pressed, and on a *trip* it half worked — the legs
    // land at different moments, so a second click took back the ones still
    // searching and re-asked the ones that had landed. See
    // [`crate::map::search::Search::Stop`].
    let (button, stopped) = ui
        .horizontal(|ui| {
            let button = ui
                .add_enabled_ui(asked.is_some(), |ui| {
                    egui::Button::new(atoms).atom_ui(ui)
                })
                .inner;
            let stopped = *plot == Plot::Working
                && ui.button(STOP).on_hover_text("Stop searching").clicked();
            (button, stopped)
        })
        .inner;
    if stopped {
        searched.write(Search::Stop);
    }
    // A route is worked out against a database that takes as long as it
    // takes, and a button that has gone quiet says nothing about whether it
    // heard.
    if let Some(turning) = button.rect(slot) {
        egui::Spinner::new().paint_at(ui, turning);
    }
    // What the button means while something is running: asking again is
    // asking, never cancelling. A leg already under way is left to finish —
    // the same question twice is one question — and a leg the form has
    // moved off is dropped for the new one.
    if *plot == Plot::Working {
        button.response.clone().on_hover_text(format!(
            "Searching. Plot again to ask for the route as it now \
                 stands, or press {STOP} to stop"
        ));
    }
    // How far the search has got, beside the button that asked for it. The
    // readings and what they are for are [`searching_says`]'s.
    if *plot == Plot::Working {
        searching_says(ui, searching, how, tune, *drive);
    }

    if (button.response.clicked() || submitted)
        && let Some((stops, range)) = asked
    {
        *plot = match plotting(range, *drive, boosts) {
            Ok(range) => {
                searched.write(Search::Route {
                    how: *how,
                    stops: asked_in_order(
                        &stops,
                        selection,
                        shape,
                        search.tour.then_some(range),
                    ),
                    // Back to text, since a route is fetched under a key
                    // made of what was asked for and a float is no kind of
                    // key.
                    range: range.to_string(),
                    drive: *drive,
                });
                Plot::Working
            }
            Err(trouble) => Plot::Failed(trouble.to_owned()),
        };
    }

    box_
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::selection::Picked;

    use crate::ui::testing::{address_of, body, holding, strung_out};

    /// A body picked out is no system, so it is not offered a route or filter
    ///
    /// Both are questions about places. Counting a body among them would put
    /// a filter on the map naming one system where two things are held, and
    /// offer a route to somewhere that is not a destination.
    #[test]
    fn a_picked_body_is_not_counted_among_the_systems() {
        let mut selection = holding(&["SOL"]);
        selection.toggle(body(3, "SOL 3", 0.));

        assert_eq!(selection.addresses(), vec![address_of("SOL")]);
        assert!(stops_of(&selection).is_err());
    }

    /// A selection holding a system at each of `places`
    fn scattered(places: &[DVec3]) -> Selection {
        let mut selection = Selection::default();
        for (address, at) in places.iter().enumerate() {
            selection.toggle(Picked::System(
                crate::map::galaxy::tests::placed(address as i64, *at),
            ));
        }
        selection
    }

    /// How wide the systems at `along` stand
    fn wide(along: &[f64]) -> Option<f64> {
        across(&strung_out(along))
    }

    /// A pair is said to be as far apart as they stand
    ///
    /// The whole of what there is to say about two: a route between them runs
    /// from the one to the other however it gets there.
    #[test]
    fn a_pair_is_as_far_apart_as_it_stands() {
        assert_eq!(wide(&[3., 15.]), Some(12.));
    }

    /// And a longer set is said by how much sky it covers
    ///
    /// Across the whole of them, from the middle of what they span to
    /// whichever is furthest and out the other side. Which is not the legs
    /// added up: the set below covers thirty light years however it is
    /// flown, where walking it end to end is thirty and doubling back over
    /// it is more.
    #[test]
    fn a_longer_set_is_as_wide_as_the_sky_it_covers() {
        assert_eq!(wide(&[0., 30., 10., 20.]), Some(30.));
        // The order it was picked in says nothing about it.
        assert_eq!(wide(&[30., 0., 20., 10.]), Some(30.));
        // Nor does a system standing inside the span.
        assert_eq!(wide(&[0., 30., 15.]), Some(30.));
    }

    /// Taking a system away never makes the set wider
    ///
    /// The figure is over pairs, so dropping a system drops its pairs and
    /// leaves the rest as they were. Measured instead from the middle of the
    /// box the systems fill -- which is what [`spanned`] does for the camera
    /// -- this set read 100.9 Ly and read **114.5** once the third system was
    /// taken out of it: losing that one let the box's middle slide, and left
    /// the far corners further from the new middle than anything had been
    /// from the old one.
    #[test]
    fn taking_a_system_away_does_not_widen_the_set() {
        let places = [
            DVec3::new(-7.1, -35.3, 28.5),
            DVec3::new(10.1, 27.3, 26.9),
            DVec3::new(-48.9, -0.4, 9.8),
            DVec3::new(0.6, -33.2, -3.2),
            DVec3::new(-13.7, -49.9, -30.6),
            DVec3::new(46.7, -5.8, 14.1),
        ];
        let whole = across(&scattered(&places)).expect("a span");
        for dropped in 0..places.len() {
            let mut fewer = places.to_vec();
            fewer.remove(dropped);
            let after = across(&scattered(&fewer)).expect("a span");
            assert!(
                after <= whole,
                "dropping #{dropped} widened {whole:.1} to {after:.1}"
            );
        }
    }

    /// A set with nothing to span is not measured at all
    ///
    /// One system spans nothing, and the form has already said it wants
    /// another.
    #[test]
    fn a_set_that_cannot_be_routed_is_not_measured() {
        assert_eq!(wide(&[]), None);
        assert_eq!(wide(&[3.]), None);
    }

    /// A trip of several says how many legs it is
    ///
    /// Two systems are apart. More stand across a span, and are said as how
    /// many legs the trip will be as well: the figure is no longer a gap
    /// between two things. The legs are [`legs_flown`]'s to count.
    #[test]
    fn a_longer_route_is_said_in_legs() {
        assert_eq!(apart_said(12., 1), "12.0 Ly apart");
        assert_eq!(apart_said(30., 3), "3 legs, 30.0 Ly across");
        assert_eq!(apart_said(4., 0), "4.0 Ly apart");
    }

    /// A distance is what the range field is for
    #[test]
    fn a_range_is_a_distance() {
        assert_eq!(jump_range("10"), Ok(10.));
        assert_eq!(jump_range("10.5"), Ok(10.5));
    }

    /// Room around what was typed is not what was meant by it
    #[test]
    fn a_range_may_be_typed_with_room_around_it() {
        assert_eq!(jump_range("  10  "), Ok(10.));
    }

    /// Anything that is not a number is not a range
    #[test]
    fn a_range_that_is_not_a_number_is_refused() {
        assert!(jump_range("far").is_err());
        assert!(jump_range("10 Ly").is_err());
    }

    /// A ship that jumps nowhere plots no route
    ///
    /// Both of these parse, so nothing but asking what the number means
    /// would catch them.
    #[test]
    fn a_range_of_nothing_or_less_is_refused() {
        assert!(jump_range("0").is_err());
        assert!(jump_range("-5").is_err());
    }

    /// A supercharged route wants a supercharge table, and says so
    ///
    /// Reported: the table was deleted and the map plotted anyway, handing
    /// back the unaided route — under the supercharged drive's name, a route
    /// carrying what it was plotted with and its panel saying so. So the map
    /// claimed to have plotted something it had not, and the only tell was a
    /// jump count that looked high.
    ///
    /// An empty table is a different answer and not this one: published with
    /// nothing in it says there is nowhere to supercharge, and the unaided
    /// route is right.
    #[test]
    fn a_supercharged_route_is_refused_without_a_table_to_plot_it() {
        let absent = galos_route::Boosts::absent();
        let published = galos_route::Boosts::of(Vec::new());

        // Unaided asks nothing of the table either way.
        assert_eq!(plotting("50", Drive::Unaided, &absent), Ok(50.));
        assert_eq!(plotting("50", Drive::Unaided, &published), Ok(50.));

        // A drive that can take a jet cone cannot be answered without one.
        for drive in [Drive::Standard, Drive::Optimised] {
            assert!(
                plotting("50", drive, &absent).is_err(),
                "{drive:?} plotted with no table to plot it from"
            );
            assert_eq!(
                plotting("50", drive, &published),
                Ok(50.),
                "{drive:?} refused against a table that is simply empty"
            );
        }

        // And the range is still asked first, so the nearer trouble is the
        // one reported.
        assert!(plotting("far", Drive::Standard, &published).is_err());
    }

    /// The stops go out in the order they were picked, unless the map is asked
    ///
    /// Which is the whole difference between a trip through stops and a set
    /// of destinations: the first is an itinerary the user wrote and the
    /// second is a question about which way round is cheapest.
    #[test]
    fn the_stops_go_out_in_the_order_they_were_picked() {
        let picked = strung_out(&[20., 0., 10., 30.]);
        let stops: Vec<&str> = stops_of(&picked).expect("stops");

        assert_eq!(
            asked_in_order(&stops, &picked, Shape::FromFirst, None),
            vec!["TEST 0", "TEST 1", "TEST 2", "TEST 3"]
        );
    }

    /// And in the cheapest order where it was
    ///
    /// The first stays where it was put, a trip having to set out from
    /// somewhere; the rest are reached whichever way costs the fewest jumps.
    #[test]
    fn a_cheapest_order_reaches_them_all_the_short_way() {
        let picked = strung_out(&[20., 0., 10., 30.]);
        let stops: Vec<&str> = stops_of(&picked).expect("stops");

        let asked =
            asked_in_order(&stops, &picked, Shape::FromFirst, Some(10.));

        assert_eq!(asked[0], "TEST 0");
        assert_eq!(asked, vec!["TEST 0", "TEST 3", "TEST 2", "TEST 1"]);
    }

    /// A trip asked for as a loop is asked for the leg home as well
    ///
    /// The stops are what the legs are cut from, so the way to ask for the
    /// flight home is to name the first stop again at the end. Which is what
    /// makes a loop nothing special to anything downstream: the leg home is
    /// walked, drawn, rowed and costed as the others are.
    ///
    /// In the order picked here, since a loop is a shape rather than an
    /// ordering: closing a trip the user ordered themselves is a run out and
    /// back the way they asked for.
    #[test]
    fn a_looping_trip_comes_home_to_where_it_set_out() {
        let picked = strung_out(&[20., 0., 10., 30.]);
        let stops: Vec<&str> = stops_of(&picked).expect("stops");

        assert_eq!(
            asked_in_order(&stops, &picked, Shape::Loop, None),
            vec!["TEST 0", "TEST 1", "TEST 2", "TEST 3", "TEST 0"]
        );
    }

    /// The two flags the form holds come to one shape
    ///
    /// A loop is only asked for where there is a trip to close: the control
    /// is not offered under two stops, and a flag left standing from a wider
    /// set is not obeyed either — nobody could see the control to let go of
    /// it. And a loop holds its start whatever the free-start flag says,
    /// every way round a ring costing the same.
    #[test]
    fn a_loop_is_asked_for_only_where_there_is_a_trip_to_close() {
        let asking = |looping, any_start, stops| {
            let fields = BarFields { looping, any_start, ..default() };
            shape_of(&fields, stops)
        };

        assert_eq!(asking(true, false, 3), Shape::Loop);
        assert_eq!(asking(true, true, 3), Shape::Loop);
        assert_eq!(asking(true, false, 2), Shape::FromFirst);
        assert_eq!(asking(true, true, 2), Shape::Anywhere);
        assert_eq!(asking(false, false, 3), Shape::FromFirst);
        assert_eq!(asking(false, true, 3), Shape::Anywhere);
    }

    /// And the legs it is flown in count the leg home
    #[test]
    fn a_loop_is_a_leg_longer_than_the_line_through_the_same_stops() {
        assert_eq!(legs_flown(3, Shape::FromFirst), 2);
        assert_eq!(legs_flown(3, Shape::Loop), 3);
        assert_eq!(legs_flown(0, Shape::Loop), 0);
    }

    /// A route runs through the systems picked out on the map
    #[test]
    fn a_route_runs_between_what_is_picked_out() {
        assert_eq!(
            stops_of(&holding(&["SOL", "SOLATI"])),
            Ok(vec!["SOL", "SOLATI"])
        );
    }

    /// In the order they were picked, the first of them being where it starts
    ///
    /// The two are told apart by nothing but that order, so a set read out in
    /// any other one plots the route backwards half the time.
    #[test]
    fn the_first_picked_is_where_a_route_starts() {
        assert_eq!(
            stops_of(&holding(&["SOLATI", "SOL"])),
            Ok(vec!["SOLATI", "SOL"])
        );
    }

    /// With nothing picked out there are no ends to run between
    #[test]
    fn a_route_with_nothing_picked_out_has_no_ends() {
        assert!(stops_of(&Selection::default()).is_err());
    }

    /// Nor with one, which is an end and no route
    #[test]
    fn a_route_out_of_one_system_is_refused() {
        assert!(stops_of(&holding(&["SOL"])).is_err());
    }

    /// A longer set is a route through the whole of it, in the order picked
    ///
    /// Every one of them is a stop rather than the first two being ends and
    /// the rest going unsaid: a set gathered out on the map is flown in the
    /// order it was gathered.
    #[test]
    fn a_longer_set_is_a_route_through_all_of_it() {
        assert_eq!(
            stops_of(&holding(&["SOL", "SOLATI", "SOLLARO"])),
            Ok(vec!["SOL", "SOLATI", "SOLLARO"])
        );
    }

    /// Each reason to refuse says something of its own
    ///
    /// They are read out of the one line, so a form that answered having
    /// picked nothing and having picked one alike would leave the user with
    /// no way to tell what it is still waiting for.
    #[test]
    fn the_reasons_are_told_apart() {
        let (nothing, single) = (Selection::default(), holding(&["SOL"]));
        let none = stops_of(&nothing);
        let one = stops_of(&single);

        assert!(none.is_err(), "{none:?}");
        assert!(one.is_err(), "{one:?}");
        assert_ne!(none, one);
    }
}
