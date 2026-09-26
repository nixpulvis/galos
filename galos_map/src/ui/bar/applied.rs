//! The filters being applied, as rows under the bar, and how much of the sky
//! gets through them
//!
//! Grouped into sections, the routes under the rest and a trip's legs under
//! the trip, and each section with a row standing for all of it.

use crate::map::filter::{Filter, Filters, Plotted};
use crate::map::galaxy::InReach;
use crate::map::route::ARROW;
use crate::ui::DOT;
use crate::ui::bar::rows::{
    Buttons, buttons_width, lay_out_buttons, lay_out_close, lay_out_replot,
    place_buttons, row_of,
};
use crate::ui::bar::{ROW_MARGIN, ROW_PADDING};
use crate::ui::list::{
    RowGesture, asked_of_row, gathering_with, settled_click,
};
use crate::ui::panels::Panels;
use crate::ui::text::{characters, shortened, thousands};
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::Ui;
use galos_route::graph::{Drive, Routing, Tuning};

/// Say which filters are being applied, and how much is getting through
///
/// Drawn whether or not the form is out. A filter changes what the whole map
/// looks like and outlives the asking, so it has to be readable from the
/// closed bar: a sky gone dim with nothing to say why is a map that looks
/// broken.
///
/// Each row is the control that turns its own filter off, so one can be
/// lifted to see what it was hiding and put back without being typed again.
/// The mark at the end takes it away for good. Over two or more, [`whole_set`]
/// stands above them and says both of those things about all of them at once.
///
/// Drawn in sections, one to a [`Section`], each with its own count standing
/// over it. A route is a line across the map and a faction is a way of reading
/// the sky, and a column that ran the two together said "3 filters" over a
/// heap of both and gave the user nowhere to turn all of one kind off.
///
/// Laid out and painted rather than assembled from widgets, as the selection
/// row is and for the same reason. A checkbox and a button under one row are
/// three things bidding for the pointer, and it flickers between being a
/// control and not as the pointer crosses them.
///
/// How much of the sky is getting through them is said by [`reaching`], which
/// stands under these and is drawn whether or not any of them is.
pub(super) fn applied(
    ui: &mut Ui,
    filters: &mut Filters,
    panels: &mut Panels,
    place: &mut usize,
) -> RowAsk {
    let mut ask = RowAsk::default();
    if filters.is_empty() {
        return ask;
    }

    // Settled after the sections are drawn, since the rows are drawn from the
    // same filters they change.
    let mut toggling = None;
    let mut removing = None;
    let mut opening = None;
    let mut whole: Option<(FilterAction, Section, Vec<usize>)> = None;

    // One row over a group of them, where the group has one. Handed the rows
    // it stands for so that what its gestures reach is what was drawn under
    // it; see [`Section::standing`] for which groups have one at all.
    let standing =
        |ui: &mut Ui,
         section: &Section,
         rows: &[usize],
         place: &mut usize,
         whole: &mut Option<(FilterAction, Section, Vec<usize>)>| {
            if section.standing(filters)
                && let Some(asked) = whole_set(
                    ui,
                    &section.said(section.counted(filters)),
                    section.hops(filters),
                    section.on(filters),
                    matches!(section, Section::Trip { .. }),
                    section.unfound(filters),
                    place,
                )
            {
                *whole = Some((asked, section.clone(), rows.to_vec()));
            }
        };

    // The sky first, then what is drawn over it, which is the order the map
    // is built up in and the order the two read in.
    let sky = Section::Filters.rows(filters);
    if !sky.is_empty() {
        standing(ui, &Section::Filters, &sky, place, &mut whole);
        section_rows(
            ui,
            filters,
            &sky,
            place,
            &mut toggling,
            &mut removing,
            &mut opening,
            &mut ask,
        );
    }

    // Then the count over every route drawn, trips and loose alike. It draws
    // no rows of its own: what it stands over is drawn below it, and drawing
    // them here as well would say each of them twice.
    let routes = Section::Routes.rows(filters);
    if !routes.is_empty() {
        standing(ui, &Section::Routes, &routes, place, &mut whole);
    }

    // And the routes themselves, in the order they were plotted. A trip
    // stands where its first leg does, since that is where the user asked for
    // it: a route plotted before it reads above it, and one plotted after
    // reads below, which is what the bar says everywhere else.
    //
    // Its legs are drawn in from its row, so the block reads as one trip with
    // its legs under it rather than as a row and then some routes. Gathered
    // rather than taken as they come, a leg that landed after something else
    // was plotted belonging with the rest of its trip; told once, since the
    // legs after the first are drawn with it.
    let mut told: Vec<Section> = Vec::new();
    for (index, active) in filters.iter().enumerate() {
        if !active.filter.is_route() {
            continue;
        }
        let Some(trip) = Section::of(&active.filter) else {
            section_rows(
                ui,
                filters,
                &[index],
                place,
                &mut toggling,
                &mut removing,
                &mut opening,
                &mut ask,
            );
            continue;
        };
        if told.contains(&trip) {
            continue;
        }

        let legs = trip.rows(filters);
        standing(ui, &trip, &legs, place, &mut whole);
        ui.indent(("trip-legs", index), |ui| {
            section_rows(
                ui,
                filters,
                &legs,
                place,
                &mut toggling,
                &mut removing,
                &mut opening,
                &mut ask,
            );
        });
        told.push(trip);
    }

    if let Some(index) = toggling {
        filters.toggle(index);
    }
    if let Some(index) = removing {
        filters.remove(index);
    }
    if let Some(filter) = opening {
        panels.open_filter(filter);
    }
    match whole {
        // Every stop of the set, in the order they are flown, each once. A
        // trip is legs and a leg is two ends, so the stop one leg lands on is
        // where the next sets out from and is one stop rather than two.
        Some((FilterAction::Select(gathering), _, rows)) => {
            let legs: Vec<Filter> = rows
                .iter()
                .filter_map(|index| filters.get(*index))
                .map(|active| active.filter.clone())
                .collect();
            ask.picked =
                Some((crate::map::filter::trip_stops(&legs), gathering));
            // And every line of it, so a trip picked out stands in front the
            // way a route picked out on its own does. Its legs are routes and
            // nothing else in a section is, so what is not one is left out
            // rather than picked out as a route that cannot be drawn.
            ask.chosen = legs.into_iter().filter(Filter::is_route).collect();
        }
        Some((FilterAction::Toggle, _, rows)) => filters.toggle_all(&rows),
        // Every filter of the section at once, so what the camera stands back
        // to take in is all of them together rather than each in turn.
        Some((FilterAction::Frame, _, rows)) => {
            ask.framed_all = rows
                .iter()
                .filter_map(|index| filters.get(*index))
                .map(|active| active.filter.clone())
                .collect();
        }
        // Every leg of the trip asked again, from nothing: a trip is one
        // route flown in several searches, and restarting it is restarting
        // each of them. Only its legs — nothing else in a section was
        // plotted.
        Some((FilterAction::Replot, _, rows)) => {
            ask.replot = rows
                .iter()
                .filter_map(|index| filters.get(*index))
                .map(|active| active.filter.clone())
                .filter(Filter::is_route)
                .collect();
        }
        // The trip as one route, which is what a panel about it is about. Its
        // legs are what it is made of and each has a panel of its own.
        Some((FilterAction::Describe, Section::Trip { stops, .. }, rows)) => {
            let legs: Vec<Filter> = rows
                .iter()
                .filter_map(|index| filters.get(*index))
                .map(|active| active.filter.clone())
                .collect();
            ask.described =
                as_one(&stops, &rows, filters).map(|whole| (whole, legs));
        }
        Some((FilterAction::Describe, ..)) => {}
        Some((FilterAction::LetGo, _, rows)) => filters.clear(&rows),
        None => {}
    }

    ask
}

/// What a press on a filter's row asked of it, beyond what the row settles
///
/// The two that reach past the filters themselves: which one the user means,
/// and where they want the camera. Handed back rather than acted on here, as
/// the selection rows hand back what they were asked, since neither is the
/// row's own business to carry out.
#[derive(Default)]
pub(super) struct RowAsk {
    /// The filters a click picked out as the ones being worked with
    ///
    /// One for a row of its own, and every leg for a trip's row: a trip is
    /// picked out as the one thing it was plotted as, so every line of it
    /// stands in front rather than one of them.
    pub(super) chosen: Vec<Filter>,
    /// The filter a double click asked to see the whole of
    pub(super) framed: Option<Filter>,
    /// The trip a section's row asked for a panel about
    ///
    /// The route it is flown as, and the legs it is made of: the first is
    /// what the panel is about and the second is how its list is broken up.
    pub(super) described: Option<(Filter, Vec<Filter>)>,
    /// Every filter of a section, where its own row asked to see them all
    ///
    /// Apart from [`Self::framed`] because it is a set rather than one of
    /// them: the camera stands back to take in all of them together, which is
    /// not where it would stand for any one.
    pub(super) framed_all: Vec<Filter>,
    /// The systems a click asked to pick out, and whether as well as instead
    ///
    /// What a route or a whole trip was plotted between, in the order it is
    /// flown. The flag is the modifier: held, the stops are picked out
    /// alongside whatever was already, which is a union and not a toggle —
    /// see [`crate::map::selection::Selection::gather`].
    pub(super) picked: Option<(Vec<i64>, bool)>,
    /// The routes a press on a row's own mark asked for again, from nothing
    ///
    /// One for a leg's row and every leg for a trip's, a trip being one route
    /// flown in several searches. Handed back rather than acted on here, as
    /// the rest are: what a search costs and what it takes back is the
    /// fetch's business. See [`crate::map::search::Search::Replot`].
    pub(super) replot: Vec<Filter>,
}

/// A trip's legs joined back into the one route they are flown as
///
/// Every system it passes through, in the order it passes through them, with
/// the seams closed: the stop a leg lands on is the stop the next sets out
/// from and stands in the list once. So a panel about it lists the trip as it
/// would list a route, and the distance each line ends in is the jump that
/// reaches that system whichever leg it fell in.
///
/// Nothing until a leg has landed. A trip whose legs are all still being
/// walked has no systems to describe, and the row already says how far along
/// they are.
///
/// The range comes off the legs, they having all been plotted for the one
/// ship. The trip it names is its own, so a panel about a trip is one panel
/// however often the row is pressed.
pub(crate) fn as_one(
    trip: &str,
    rows: &[usize],
    filters: &Filters,
) -> Option<Filter> {
    let legs: Vec<&crate::map::filter::Entry> =
        rows.iter().filter_map(|index| filters.get(*index)).collect();
    let asked = legs.first()?;
    let range = asked.filter.range()?.to_owned();
    // As the range is, and for the same reason: the legs were all plotted for
    // the one ship, so the trip they come to was plotted for it too. The
    // search mode with them, all the legs having been asked the one way.
    let drive = asked.filter.drive()?;
    let how = asked.filter.how()?;
    // And how they were planned, which is the same for all of them for the
    // same reason: one ask, one set of settings, however many legs it came
    // to. A leg that was never planned carries what it was asked with all
    // the same, so the trip's own filter matches its legs'.
    let tune = match &asked.filter {
        Filter::Route { tune, .. } => Some(*tune),
        _ => None,
    }?;

    // What has landed, and only that. A leg still being searched — or one
    // stopped, or one with no route to be found — has a row carrying the two
    // ends it was asked between, and joining those in would have the trip run
    // through a jump nobody flew. The row over it already says how many legs
    // are still out; see [`crate::ui::panels`]'s summary.
    let mut systems: Vec<i64> = Vec::new();
    for leg in legs.iter().filter(|leg| leg.landed()) {
        let Filter::Route { systems: hops, .. } = &leg.filter else { continue };
        let seam = usize::from(!systems.is_empty());
        systems.extend(hops.iter().skip(seam));
    }
    if systems.len() < 2 {
        return None;
    }

    Some(Filter::Route {
        label: Section::trip_said(trip),
        systems,
        range,
        trip: Some(trip.to_owned()),
        drive,
        how,
        tune,
    })
}

/// A trip's panel as its legs now stand, and the legs it is made of
///
/// **A trip is plotted a leg at a time, so a panel opened before the last
/// of them lands describes a route that is not finished.** It used to
/// describe it *once*, when it was opened: the joined filter was built
/// there and kept, so a leg landing afterwards changed nothing and the
/// panel went on saying a partial trip's systems, distance and longest jump
/// as though they were the whole of it. Rebuilt here every frame instead,
/// off whatever legs the bar now holds.
///
/// [`None`] where `filter` is not a trip's joined route, or where its legs
/// have gone: a trip whose rows were closed has nothing left to describe,
/// and the panel keeps what it last had rather than emptying.
pub(crate) fn trip_now(
    filter: &Filter,
    filters: &Filters,
) -> Option<(Filter, Vec<Filter>)> {
    let trip = filter.trip()?;
    let section = Section::of(filter)?;
    let rows = section.rows(filters);
    let legs: Vec<Filter> = rows
        .iter()
        .filter_map(|index| filters.get(*index))
        .map(|active| active.filter.clone())
        .collect();

    Some((as_one(trip, &rows, filters)?, legs))
}

/// Which group of the bar's filter rows a filter stands in
///
/// Routes apart from the rest. A route is a line drawn across the map between
/// two systems the user named, and a faction or a hand-picked set is a way of
/// reading the sky it is drawn over. They are worth different questions: how
/// many routes am I comparing, and how much of the sky am I picking out.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Section {
    /// Factions and hand-picked sets, which pick the sky out
    Filters,
    /// The legs of one plotted trip, which are flown as one thing
    ///
    /// A trip through five systems is four routes, and they are four rows.
    /// Grouped so the four read as the one trip they were asked for: the row
    /// over them says what the whole of it comes to, framing takes the legs
    /// together, and turning it off takes the whole line off the map.
    ///
    /// The stops and the ship, rather than the stops alone. The same stops
    /// plotted again for a ship that reaches further are a second trip
    /// through them — different lines, different rows, and a second answer
    /// to compare against the first. Keyed on the name alone, the second
    /// plot's legs fell in among the first's: one row saying "2 Leg Route"
    /// over four legs, framing that took both trips together, and a panel
    /// that read one trip's systems at the other's range.
    Trip {
        /// The trip's stops, joined by [`ARROW`], as the user typed them
        stops: String,
        /// How far the ship it was plotted for reaches, as it was typed
        range: String,
        /// Which drive it was plotted for
        drive: Drive,
        /// How hard the search was asked to work at it
        how: Routing,
        /// How the plan over the boost stars was worked out
        ///
        /// Part of what a trip is for the reason the rest are: the same
        /// stops planned over different gaps are two trips and two sets of
        /// rows, not one that swallowed the other's legs.
        tune: Tuning,
    },
    /// The count standing over every route drawn, trips and loose alike
    ///
    /// A trip counts once, being one route flown in several legs: two trips
    /// and two routes asked for on their own are four routes, and the row
    /// says so. Its gestures reach all of them — every line off the map,
    /// every route framed at once, every one let go of — which is what a
    /// count over some of the routes could not say.
    ///
    /// Drawn above the routes rather than among them, being the count over
    /// all of them. What it stands over is drawn under it in the order it
    /// was plotted: the trips as blocks, each where its first leg fell, and
    /// the routes asked for on their own — [`Section::Loose`] — each where
    /// it fell.
    Routes,
    /// Routes asked for on their own, which are drawn over the sky
    ///
    /// Rows and no count of their own. What stands over them is
    /// [`Section::Routes`], which counts the trips beside them.
    Loose,
}

impl Section {
    /// Every trip the bar has legs for, in the order they were first plotted
    ///
    /// A trip stands where its first leg does, that being where the user
    /// asked for it. Only trips: what stands over all of them is
    /// [`Section::Routes`], and the sky and the loose routes are not trips.
    ///
    /// Only trips with a leg in them. A trip standing over nothing is a count
    /// of nothing.
    fn trips(filters: &Filters) -> Vec<Section> {
        let mut trips: Vec<Section> = Vec::new();
        for active in filters.iter() {
            let Some(trip) = Section::of(&active.filter) else { continue };
            if !trips.contains(&trip) {
                trips.push(trip);
            }
        }
        trips
    }

    /// The trip section `filter` belongs to, where it is a leg of one
    ///
    /// Nothing for a route asked for on its own and nothing for a filter that
    /// is no route, neither of which is a leg of anything.
    fn of(filter: &Filter) -> Option<Section> {
        let (range, drive, how, tune) = filter.ship()?;

        Some(Section::Trip {
            stops: filter.trip()?.to_owned(),
            range: range.to_owned(),
            drive,
            how,
            tune,
        })
    }

    /// Whether this section holds `filter`
    fn holds(&self, filter: &Filter) -> bool {
        match self {
            Section::Filters => !filter.is_route(),
            // Every leg of the one plot: the same stops asked for again at
            // another range is another trip, and its legs are its own.
            Section::Trip { stops, range, drive, how, tune } => {
                filter.trip() == Some(stops.as_str())
                    && filter.ship()
                        == Some((range.as_str(), *drive, *how, *tune))
            }
            // Every route drawn, so the count over them reaches all of them
            // at once: a leg is a route, whatever else it is part of.
            Section::Routes => filter.is_route(),
            // A route belonging to a trip is drawn under that trip's row.
            Section::Loose => filter.is_route() && filter.trip().is_none(),
        }
    }

    /// Which places in `filters` this section's rows stand at
    ///
    /// Places rather than the filters themselves, since what the row over them
    /// asks for is a change to those filters and an index is what says which.
    fn rows(&self, filters: &Filters) -> Vec<usize> {
        filters
            .iter()
            .enumerate()
            .filter(|(_, active)| self.holds(&active.filter))
            .map(|(index, _)| index)
            .collect()
    }

    /// Whether any filter in this section is turned on
    fn on(&self, filters: &Filters) -> bool {
        self.rows(filters)
            .iter()
            .filter_map(|index| filters.get(*index))
            .any(|active| active.enabled)
    }

    /// How many things this section's own row stands over
    ///
    /// What it counts rather than how many rows it reaches: a trip is one
    /// route however many legs it is flown in, so the count over the routes
    /// counts each trip once and each loose route once. Everywhere else it
    /// is the rows themselves.
    fn counted(&self, filters: &Filters) -> usize {
        match self {
            Section::Routes => {
                Section::trips(filters).len()
                    + Section::Loose.rows(filters).len()
            }
            _ => self.rows(filters).len(),
        }
    }

    /// Whether this section has a row of its own over what it holds
    ///
    /// Over two or more of them: one row already says everything a count of
    /// one could, and the control over it would do what that row's own does.
    ///
    /// A trip keeps its row whatever it holds. It is named rather than
    /// counted, so the row says something no leg of it says, and a trip whose
    /// legs have not all landed yet would otherwise appear as a heap of
    /// routes and then gather itself up.
    ///
    /// The loose routes never have one. They are counted with the trips, by
    /// the row [`Section::Routes`] stands over both with, and a second count
    /// over some of them would be two rows saying different numbers about the
    /// same lines.
    fn standing(&self, filters: &Filters) -> bool {
        match self {
            Section::Loose => false,
            Section::Trip { .. } => true,
            _ => self.counted(filters) > 1,
        }
    }

    /// How many jumps the whole of this section is flown in
    ///
    /// A trip is flown in its legs, so what it comes to is their jumps added
    /// up — the figure it was plotted to find out, and the one a user
    /// comparing two plots of the same stops is comparing. The legs land one
    /// at a time, so this is what has landed: it grows as they arrive and
    /// settles when the last of them does.
    ///
    /// Nothing until a leg of it has landed, rather than a nought. A trip
    /// whose legs are all still being walked has flown no jumps and found no
    /// route, and "0 hops" over it reads as the answer having come back
    /// empty; the rows under it say what each leg is doing.
    ///
    /// Nothing for the rest. A count over the routes stands over lines that
    /// are not flown as one thing, and adding their jumps together would be a
    /// number for a journey nobody is making.
    fn hops(&self, filters: &Filters) -> Option<usize> {
        match self {
            Section::Trip { .. } => {
                let landed: Vec<usize> = self
                    .rows(filters)
                    .iter()
                    .filter_map(|index| filters.get(*index))
                    // What has landed. A leg still being searched carries the
                    // two ends it was asked between, which is a hop nobody
                    // has flown, and counting it would have a trip claim a
                    // jump per leg before a single one landed.
                    .filter(|active| active.landed())
                    .filter_map(|active| active.filter.hops())
                    .collect();
                (!landed.is_empty()).then(|| landed.iter().sum())
            }
            _ => None,
        }
    }

    /// Whether anything in this section has a search left to ask again
    ///
    /// A trip whose every leg has landed is a route that was found: there is
    /// nothing to restart, and a mark offering it would ask for the same
    /// answer a second time. One leg short of that — still searching,
    /// stopped, or with no route to be found — and the whole trip is worth
    /// asking over, its legs being one route.
    ///
    /// Only a trip. What stands over every route on the map stands over lines
    /// that were asked for separately, and [`Section::Filters`] over things
    /// that were never searched for at all.
    fn unfound(&self, filters: &Filters) -> bool {
        matches!(self, Section::Trip { .. })
            && self
                .rows(filters)
                .iter()
                .filter_map(|index| filters.get(*index))
                .any(|active| !active.landed())
    }

    /// How many legs the trip through `stops` has
    ///
    /// Its name is its stops joined by [`ARROW`], so the legs are the gaps
    /// between them: one fewer than the stops, and one for every arrow.
    fn legs(stops: &str) -> usize {
        stops.matches(ARROW).count()
    }

    /// What a row over the trip through `stops` says
    ///
    /// Here rather than inside [`Self::said`] because the panel a trip opens
    /// is named the same way, and a trip that read one thing in the bar and
    /// another over its panel would be two things.
    fn trip_said(stops: &str) -> String {
        let legs = Section::legs(stops);
        if legs == 1 {
            "1 Leg Route".to_owned()
        } else {
            format!("{legs} Leg Route")
        }
    }

    /// What a row standing over `count` of them says
    ///
    /// A trip says how many legs it is rather than naming its stops. The
    /// stops are the rows under it, named there and in that order, and a
    /// trip through six of them spelled out runs longer than the bar is
    /// wide. What it comes to is added on separately, by whoever has the legs
    /// to add up.
    fn said(&self, count: usize) -> String {
        match self {
            Section::Filters => {
                if count == 1 {
                    "1 filter".to_owned()
                } else {
                    format!("{count} filters")
                }
            }
            Section::Trip { stops, .. } => Section::trip_said(stops),
            Section::Routes | Section::Loose => {
                if count == 1 {
                    "1 route".to_owned()
                } else {
                    format!("{count} routes")
                }
            }
        }
    }
}

/// How many jumps a route is flown in, as it is said anywhere it is said
///
/// What a route says at the end of its row, faint beside the name, where a
/// selection's row says how far off its system is — and what a leg's own
/// heading says in a trip's panel, the panel being about the same legs the
/// bar has rows for. One place rather than one per kind of row: a trip's
/// row says the same about the whole of it as its legs' rows say about each
/// of them, and the two reading differently would be two answers to the one
/// question.
pub(crate) fn hops_said(hops: usize) -> String {
    match hops {
        1 => "1 hop".to_owned(),
        hops => format!("{hops} hops"),
    }
}

/// The same reading, laid out to paint in a row of the bar
fn hops_galley(ui: &Ui, hops: usize) -> std::sync::Arc<egui::Galley> {
    faint_galley(ui, &hops_said(hops))
}

/// What a row says at its far end, laid out faint beside the name
///
/// The hops a route is flown in, or — where its search never landed one —
/// what became of that search: see [`crate::map::filter::Plotted::said`].
fn faint_galley(ui: &Ui, said: &str) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(egui::RichText::new(said).weak()).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    )
}

/// Draw a row for each filter at `rows`, and say what a click asked of one
///
/// Split out from [`applied`] because the sections draw the same row and only
/// the count above them differs.
#[allow(clippy::too_many_arguments)]
fn section_rows(
    ui: &mut Ui,
    filters: &Filters,
    rows: &[usize],
    place: &mut usize,
    toggling: &mut Option<usize>,
    removing: &mut Option<usize>,
    opening: &mut Option<Filter>,
    ask: &mut RowAsk,
) {
    let gap = ui.spacing().item_spacing.x;

    for index in rows.iter().copied() {
        let Some(active) = filters.get(index) else { continue };
        // What a route says at its end, where a selection row says how far
        // off its system is. Nothing on the others: a faction's name says all
        // there is to say, and a set says how many it holds in its own name.
        //
        // For a leg whose search has not landed a route, what became of that
        // search instead of a hop count. Its row carries the two ends it was
        // asked between, which `hops` would read as a one-jump route — so a
        // leg still being walked would say "1 hop" over a route nobody has
        // found. See [`crate::map::filter::Plotted`].
        let tail = match active.plotted.and_then(Plotted::said) {
            Some(said) => Some(faint_galley(ui, said)),
            None => active.filter.hops().map(|hops| hops_galley(ui, hops)),
        };

        // No info button where nothing could be said: a span admits the
        // galaxy over and what it admits is on the map already. And the one
        // that asks again only where there is a search left to ask again: a
        // route that landed is the answer to its own question, and asking it
        // over would put the same question a second time. A faction was
        // never searched for at all.
        let buttons = if active.filter.is_route() && !active.landed() {
            lay_out_replot(ui)
        } else if active.filter.worth_describing() {
            lay_out_buttons(ui)
        } else {
            lay_out_close(ui)
        };

        // Whatever the dot, the hops and the marks leave. Faction names run
        // long, and one laid out against no bound is painted out past the
        // edge of the bar.
        let room = ui.available_width()
            - ROW_PADDING * 2.
            - DOT
            - gap
            - buttons_width(&buttons, gap)
            - tail.as_ref().map_or(0., |tail| tail.size().x + gap);
        // Cut here rather than left to the layout below, which cuts from the
        // right hand end and would take the far end of a route with it.
        // Egui's own truncation still stands behind this, for the names it
        // has nothing to say about.
        let text = egui::RichText::new(shortened(
            active.filter.name(),
            characters(ui.ctx(), egui::TextStyle::Body, room),
        ));
        let name = egui::WidgetText::from(if active.enabled {
            text.strong()
        } else {
            text.weak()
        })
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            room.max(0.),
            egui::TextStyle::Body,
        );

        // By place in the bar rather than by which filter it names, counted
        // on from the selection's rows above. See [`row_of`].
        let of = ("bar-row", *place);
        *place += 1;
        let (outer, row) = row_of(
            ui,
            name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
            of,
        );
        let rect = outer.shrink2(egui::vec2(0., ROW_MARGIN));

        if row.hovered() || row.has_focus() {
            ui.painter().rect_filled(
                rect,
                ui.visuals().widgets.hovered.corner_radius,
                ui.visuals().widgets.hovered.weak_bg_fill,
            );
        }

        // Filled while the filter is being asked and hollow while it is not,
        // so that a filter turned off still reads as one that is there.
        let middle = rect.center().y;
        let mut x = rect.left() + ROW_PADDING;
        let dot = egui::pos2(x + DOT / 2., middle);
        if active.enabled {
            ui.painter().circle_filled(
                dot,
                DOT / 2.,
                ui.visuals().strong_text_color(),
            );
        } else {
            ui.painter().circle_stroke(
                dot,
                DOT / 2.,
                egui::Stroke::new(1_f32, ui.visuals().weak_text_color()),
            );
        }
        x += DOT + gap;
        for galley in [Some(name), tail].into_iter().flatten() {
            let size = galley.size();
            // The galleys carry the colors they were laid out in, so there
            // is nothing for a fallback to answer for.
            ui.painter().galley(
                egui::pos2(x, middle - size.y / 2.),
                galley,
                egui::Color32::PLACEHOLDER,
            );
            x += size.x + gap;
        }

        let Buttons { info, close, again } =
            place_buttons(ui, rect, buttons, of);

        // The dot is the switch. It is already the answer to whether the
        // filter is being asked -- filled while it is, hollow while it is not
        // -- so the thing that says so is the thing that changes it, and the
        // row is left for what the row is about.
        //
        // Interacted after the row was laid out, so it stands in front and
        // takes the press the row would otherwise have.
        let switch = ui.interact(
            egui::Rect::from_center_size(dot, egui::Vec2::splat(DOT * 2.)),
            ui.id().with((of, "dot")),
            egui::Sense::click(),
        );

        let settled =
            settled_click(ui, row.id, row.clicked(), row.double_clicked());
        match asked_of_row(
            close.clicked(),
            info.is_some_and(|info| info.clicked()),
            again.is_some_and(|again| again.clicked()),
            switch.clicked(),
            row.double_clicked(),
            settled.is_some(),
        ) {
            Some(RowGesture::LetGo) => *removing = Some(index),
            Some(RowGesture::Describe) => {
                *opening = Some(active.filter.clone())
            }
            Some(RowGesture::Replot) => {
                ask.replot = vec![active.filter.clone()]
            }
            Some(RowGesture::Toggle) => *toggling = Some(index),
            Some(RowGesture::Frame) => ask.framed = Some(active.filter.clone()),
            Some(RowGesture::Select) => {
                let keys = settled.unwrap_or_default();
                ask.picked =
                    Some((active.filter.stops(), gathering_with(keys)));
                ask.chosen = vec![active.filter.clone()];
            }
            None => {}
        }
        switch.on_hover_cursor(egui::CursorIcon::PointingHand);
        row.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

/// Say how much of the sky is in reach, and how much of it is getting through
///
/// Under the filter rows, and drawn whether or not there are any. What the
/// spyglass reaches is worth knowing before anything has been asked of it: it
/// is the one number that says whether the map is showing a handful of systems
/// or a hundred thousand, and the control that decides it is the one to reach
/// for either way. Said in the spyglass's own name for that reason.
///
/// Counted from [`InReach`], which is tallied where visibility is settled for
/// every system at once, so what is said is the answer the map acted on. Within
/// the spyglass rather than loaded: what it has dragged in from wherever the
/// camera has been is not what the user is looking at.
///
/// The leading number is always what can be seen. `dimming` says whether there
/// is more sky behind it that can also be seen, faintly, and only then is the
/// larger number worth putting beside it:
///
/// - Nothing asked of the map, so the two are the same number: `324 in
///   spyglass`
/// - Filters, and what they exclude drawn faintly behind: `8 of 324 in
///   spyglass`
/// - Filters, and what they exclude drawn not at all: `8 in spyglass`, the
///   rest being neither on screen nor fetched
///
/// Nothing at all for a sky of one system or none. A count of one is not a
/// reading anybody wants: it says less than the system's own name beside it
/// does, and it is what the line stands at for the whole of a descent, where
/// the reach holds the system the camera is inside and nothing else.
///
/// Said in as few words as it can be. The bar is [`BAR_WIDTH`](crate::ui::BAR_WIDTH) wide and the
/// numbers are what grow: the sky runs to millions of systems, and a line
/// that has to wrap to hold two of them is a line that moves the rows under
/// it about as the user flies.
pub(super) fn reaching(
    ui: &mut Ui,
    in_reach: &InReach,
    dimming: bool,
    spawning: bool,
    evicting: bool,
) {
    let InReach { admitted, total } = *in_reach;
    // Nothing to count where the sky in reach is one system or none. A count
    // of one says less than the system's own name does, and it is what the
    // line reads as for the whole of a descent: the camera inside a system
    // with the reach drawn in behind it has that system and nothing else.
    if total <= 1 {
        return;
    }

    let said = if dimming {
        format!(
            "{} of {} in spyglass",
            thousands(admitted as u64),
            thousands(total as u64)
        )
    } else {
        format!("{} in spyglass", thousands(admitted as u64))
    };
    // To the right of the count: a green dot while systems are still being
    // turned into stars, a red one while they are being taken back off. Drawn
    // only for the frames the queue is not empty, so they blink on and off
    // with the work rather than easing.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(said).weak());
        let radius = ui.text_style_height(&egui::TextStyle::Body) * 0.3;
        if spawning {
            dot(ui, radius, egui::Color32::from_rgb(80, 200, 120));
        }
        if evicting {
            dot(ui, radius, egui::Color32::from_rgb(220, 80, 80));
        }
    });
}

/// A filled circle of `radius` in `color`, laid inline in a row
///
/// Painted rather than lettered so it is a shape whatever the font holds, and
/// held to no animation: it is there while the work is and gone the frame it
/// stops.
fn dot(ui: &mut Ui, radius: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::splat(radius * 2.),
        egui::Sense::hover(),
    );
    ui.painter().circle_filled(rect.center(), radius, color);
}

/// What the bar can be asked to do with the filters as a set
#[derive(Clone, Copy, Debug, PartialEq)]
enum FilterAction {
    /// Pick out the systems every one of them was plotted between
    ///
    /// What a click on the row means. A section of filters has no one filter
    /// to be the one being worked with, which is what a click on a filter's
    /// own row settles, so the row is free to mean the thing a set can answer
    /// and a single row cannot: every stop of the trip at once.
    ///
    /// Carries whether the press meant "and these as well", read off the
    /// press rather than off the frame it is acted on — the click waits out
    /// the window a double could arrive in, and the modifier is let go of
    /// inside it. See [`settled_click`].
    Select(bool),
    /// Turn every filter off, or every one back on
    Toggle,
    /// Open a panel describing the whole of it
    Describe,
    /// Ask every leg of it again, from nothing
    ///
    /// A trip is one route flown in several, and restarting it means
    /// restarting its legs: they are separate searches and each is asked
    /// over. Only a trip's row offers it — the count over every route on the
    /// map stands over lines that were asked for separately, and re-asking
    /// the lot from one press is more than any press means.
    Replot,
    /// Send the camera to see the whole of what all of them admit
    Frame,
    /// Take them all away
    LetGo,
}

/// One row standing for every filter under it, and what a click on it asked
///
/// The two gestures a row gives, said of all of them at once: the row turns
/// them off and back on, and the mark at its end takes them away. Both are
/// what a set wants and what a list of rows answers slowest, a filter at a
/// time being the only way to reach them otherwise.
///
/// Drawn as a row rather than as a pair of buttons, so there is nothing new
/// to read: the dot and the mark say for all of them what each row's own say
/// for one, and they stand in the same two places.
///
/// The dot is filled while `on`, which says whether any filter under it is
/// being asked, since that is what the row undoes. The same click that put the
/// rest of the sky away brings it back, so the state it shows is the state its
/// own gesture is about.
///
/// `said` is what it is standing over, which the section works out: the bar
/// draws its filters in groups and each has one of these of its own, so this
/// is handed what to say rather than counting a whole set it is not about.
///
/// `hops` is how many jumps the whole of it is flown in, where it is flown at
/// all: a trip says at its end what each of its legs says at theirs, that
/// being the one figure a trip was plotted to find out. [`None`] for a count
/// standing over things that are not flown as one thing.
///
/// `unfound` is whether anything under it has a search left to ask again,
/// which is what the mark offering one turns on; see [`Section::unfound`].
fn whole_set(
    ui: &mut Ui,
    said: &str,
    hops: Option<usize>,
    on: bool,
    describes: bool,
    unfound: bool,
    place: &mut usize,
) -> Option<FilterAction> {
    let gap = ui.spacing().item_spacing.x;
    // A trip is one thing to be described, as each of its legs is: how far
    // the whole of it runs, and every system it passes through in the order
    // it passes through them. A count of filters is not, there being nothing
    // to say about a heap of them that their own rows do not say.
    // A trip is the one section there is anything to describe: it is one
    // route flown in legs, where a count of filters or of routes stands over
    // things that were asked for separately. And it offers to be asked again
    // only while a leg of it has not been found — a trip whose every leg
    // landed is the answer to its own question.
    let buttons = match (describes, unfound) {
        (true, true) => lay_out_replot(ui),
        (true, false) => lay_out_buttons(ui),
        _ => lay_out_close(ui),
    };

    // As a row below lays its own out: the name is cut to whatever the dot,
    // the jumps and the marks leave, so a trip too long to spell out never
    // paints over what it is flown in.
    let hops = hops.map(|hops| hops_galley(ui, hops));
    let room = ui.available_width()
        - ROW_PADDING * 2.
        - DOT
        - gap
        - buttons_width(&buttons, gap)
        - hops.as_ref().map_or(0., |hops| hops.size().x + gap);
    let text = egui::RichText::new(said);
    let name =
        egui::WidgetText::from(if on { text.strong() } else { text.weak() })
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                room.max(0.),
                egui::TextStyle::Body,
            );

    // By place in the bar, as the rows below it are keyed. See [`row_of`].
    let of = ("bar-row", *place);
    *place += 1;
    let (outer, row) = row_of(
        ui,
        name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
        of,
    );
    let rect = outer.shrink2(egui::vec2(0., ROW_MARGIN));

    if row.hovered() || row.has_focus() {
        ui.painter().rect_filled(
            rect,
            ui.visuals().widgets.hovered.corner_radius,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }

    let middle = rect.center().y;
    let mut x = rect.left() + ROW_PADDING;
    let dot = egui::pos2(x + DOT / 2., middle);
    if on {
        ui.painter().circle_filled(
            dot,
            DOT / 2.,
            ui.visuals().strong_text_color(),
        );
    } else {
        ui.painter().circle_stroke(
            dot,
            DOT / 2.,
            egui::Stroke::new(1_f32, ui.visuals().weak_text_color()),
        );
    }
    x += DOT + gap;
    // The galleys carry the colors they were laid out in, so there is nothing
    // for a fallback to answer for.
    for galley in [Some(name), hops].into_iter().flatten() {
        let size = galley.size();
        ui.painter().galley(
            egui::pos2(x, middle - size.y / 2.),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        x += size.x + gap;
    }

    let Buttons { info, close, again } = place_buttons(ui, rect, buttons, of);

    // The switch, as the rows below have. Said of all of them at once, which
    // is what this row is for.
    let switch = ui.interact(
        egui::Rect::from_center_size(dot, egui::Vec2::splat(DOT * 2.)),
        ui.id().with((of, "dot")),
        egui::Sense::click(),
    );

    // Read in the same order a row below is, by the same rule: see
    // [`asked_of_row`]. A click on the name means the systems the set was
    // plotted between — a trip's every stop, which is the one thing this row
    // can say that none of the rows under it can.
    let settled =
        settled_click(ui, row.id, row.clicked(), row.double_clicked());
    let asked = match asked_of_row(
        close.clicked(),
        info.is_some_and(|info| info.clicked()),
        again.is_some_and(|again| again.clicked()),
        switch.clicked(),
        row.double_clicked(),
        settled.is_some(),
    ) {
        Some(RowGesture::LetGo) => Some(FilterAction::LetGo),
        Some(RowGesture::Describe) => Some(FilterAction::Describe),
        Some(RowGesture::Replot) => Some(FilterAction::Replot),
        Some(RowGesture::Toggle) => Some(FilterAction::Toggle),
        Some(RowGesture::Frame) => Some(FilterAction::Frame),
        Some(RowGesture::Select) => Some(FilterAction::Select(gathering_with(
            settled.unwrap_or_default(),
        ))),
        None => None,
    };
    row.on_hover_cursor(egui::CursorIcon::PointingHand);
    switch.on_hover_cursor(egui::CursorIcon::PointingHand);
    asked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::filter::{Standstill, Watch};

    use crate::testing::{painted, words};
    use crate::ui::bar::filter::watch_control;

    use crate::ui::text::CUT;
    use crate::ui::{AGAIN, CLOSE, INFO};

    // Only the debug-only passes below use it: egui compiles its
    // between-pass id check out of a release build. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    /// Drawn filter rows for `names`
    fn draw_filters<'a>(names: &'a [&'a str]) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut filters = Filters::default();
            for name in names {
                filters.add(Filter::Faction {
                    id: name.len() as i32,
                    name: (*name).to_owned(),
                });
            }
            let mut panels = Panels::default();
            applied(ui, &mut filters, &mut panels, &mut 0);
        }
    }

    // Only the debug-only passes below use it: egui compiles its
    // between-pass id check out of a release build. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    /// Drawn filter rows for a faction and `routes` routes
    fn draw_sections<'a>(routes: usize) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut filters = Filters::default();
            filters.add(Filter::Faction { id: 1, name: "Empire".into() });
            for held in 0..routes {
                filters
                    .add(a_route(&(0..=held as i64 + 1).collect::<Vec<_>>()));
            }
            let mut panels = Panels::default();
            applied(ui, &mut filters, &mut panels, &mut 0);
        }
    }

    /// A section growing a count of its own does not hand a row its place
    ///
    /// The second route stands a "2 routes" row over them, which lands in the
    /// rectangle the first route's row was drawn in and moves that row down.
    /// The places are counted across the whole column, so the rectangle keeps
    /// its id and what stands there changes underneath it.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn a_section_gaining_its_count_does_not_change_the_row_ids() {
        let said =
            crate::testing::between_passes(draw_sections(1), draw_sections(2));

        assert!(said.is_empty(), "{said:?}");
    }

    /// And losing it does not either
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn a_section_losing_its_count_does_not_change_the_row_ids() {
        let said =
            crate::testing::between_passes(draw_sections(2), draw_sections(1));

        assert!(said.is_empty(), "{said:?}");
    }

    /// Dropping a filter is not read as a widget changing identity either
    ///
    /// The other half of what the bar does when a row goes: the rows below
    /// move up into the rectangle it left.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn dropping_a_filter_is_not_an_id_change() {
        let said = crate::testing::between_passes(
            draw_filters(&["Empire", "Federation", "Alliance"]),
            draw_filters(&["Empire", "Alliance"]),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// Falling to one filter takes the row over the set with it
    ///
    /// The row stands above the others, so losing it moves every remaining
    /// row up one place. Two rows go at once, which is the shape egui reads
    /// as a widget taking another's state if the ids do not follow the
    /// places.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn dropping_to_one_filter_does_not_change_the_row_ids() {
        let said = crate::testing::between_passes(
            draw_filters(&["Empire", "Federation"]),
            draw_filters(&["Empire"]),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// The rows drawn from `filters`, with `standstill` holding them or not
    fn drawn_rows(filters: &Filters, standstill: &Standstill) -> Vec<String> {
        words(|ui| {
            let mut standing =
                standstill.rows(filters).unwrap_or_else(|| filters.clone());
            let mut panels = Panels::default();
            applied(ui, &mut standing, &mut panels, &mut 0);
        })
    }

    /// A time filter's row offers no panel
    ///
    /// The other filters name a set of systems worth reading as a list. A span
    /// admits the galaxy over and changes while it is being read, so a panel
    /// about one is a question over every system on record answered with an
    /// arbitrary slice of the newest. What it admits is on the map already.
    #[test]
    fn a_time_filters_row_offers_no_panel() {
        let mut filters = Filters::default();
        filters.ask_within("1 day", chrono::Duration::days(1));

        let said = drawn_rows(&filters, &Standstill::default());

        assert!(said.contains(&CLOSE.to_owned()), "no way to let go: {said:?}");
        assert!(!said.contains(&INFO.to_owned()), "a panel was offered");
    }

    /// Where a faction's does
    #[test]
    fn a_faction_filters_row_offers_a_panel() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });

        let said = drawn_rows(&filters, &Standstill::default());

        assert!(said.contains(&INFO.to_owned()), "no panel offered: {said:?}");
    }

    /// The rows stand still while the time control is held
    ///
    /// They are drawn above it, so a row appearing while it is being dragged
    /// takes the control down out from under the pointer. What the drag asks
    /// for goes in as it is asked, and the sky is filtered by it; the row for
    /// it waits until the drag is over.
    #[test]
    fn the_rows_stand_still_while_the_time_control_is_held() {
        let mut before = Filters::default();
        before.add(Filter::Faction { id: 1, name: "Empire".into() });

        let mut standstill = Standstill::default();
        standstill.hold(&before);

        // What the drag asks for, which the sky is filtered by at once.
        let mut asked = before.clone();
        asked.ask_within("1 day", chrono::Duration::days(1));

        assert_eq!(
            drawn_rows(&asked, &standstill),
            drawn_rows(&before, &Standstill::default()),
            "a row landed under the pointer"
        );

        standstill.release();
        assert_ne!(
            drawn_rows(&asked, &standstill),
            drawn_rows(&before, &Standstill::default()),
            "the row never landed"
        );
    }

    /// A row already there reads live while the control is held
    ///
    /// Only its being there is held. What it says follows the drag, so a span
    /// dragged from one to the next says the span it has reached rather than
    /// the one the press landed on.
    #[test]
    fn a_held_time_filter_still_reads_live() {
        let mut asked = Filters::default();
        asked.ask_within("1 day", chrono::Duration::days(1));

        let mut standstill = Standstill::default();
        standstill.hold(&asked);

        asked.ask_within("6 hours", chrono::Duration::hours(6));

        let said = drawn_rows(&asked, &standstill);
        assert!(said.contains(&"Last 6 hours".to_owned()), "{said:?}");
        assert!(!said.contains(&"Last 1 day".to_owned()), "{said:?}");
    }

    /// A control slid to its far end says so where its row stands
    ///
    /// The row stays until the gesture is over, a row going out from under the
    /// pointer taking the control up a row the same way one arriving takes it
    /// down. What it says is what is filtering, which by then is nothing, so
    /// it reads as the control does rather than as the span it was asked as.
    #[test]
    fn a_time_filter_slid_off_says_so_before_it_goes() {
        let mut asked = Filters::default();
        asked.ask_within("1 day", chrono::Duration::days(1));

        let mut standstill = Standstill::default();
        standstill.hold(&asked);

        // Dragged to the far end, which stops asking where the row stands.
        asked.turn_time_off("Off");

        let said = drawn_rows(&asked, &standstill);
        assert!(said.contains(&"Off".to_owned()), "{said:?}");
        assert!(
            !said.contains(&"Last 1 day".to_owned()),
            "the row kept the span it had stopped asking for"
        );
    }

    /// The control's id under `rows` filter rows standing above it
    fn watch_id(rows: usize) -> egui::Id {
        let mut got = None;
        let ctx = crate::testing::context();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut filters = Filters::default();
            for id in 0..rows {
                filters.add(Filter::Faction {
                    id: id as i32,
                    name: format!("Faction {id}"),
                });
            }
            let mut panels = Panels::default();
            applied(ui, &mut filters, &mut panels, &mut 0);

            let mut watch = Watch(1);
            let mut active = Filters::default();
            let mut standstill = Standstill::default();
            got = Some(
                watch_control(ui, &mut watch, &mut active, &mut standstill).id,
            );
        });

        got.expect("the control drew")
    }

    /// The control keeps its id as rows come and go above it
    ///
    /// Egui numbers a widget by how many were drawn before it and follows a
    /// drag by id, so a row appearing above this one mid-drag would leave the
    /// user dragging a widget that no longer exists. Asking for a filter here
    /// is what adds that row, so the control moves the moment it is used.
    #[test]
    fn the_time_control_keeps_its_id_under_rows() {
        assert_eq!(watch_id(0), watch_id(1), "one row renamed the control");
        assert_eq!(watch_id(1), watch_id(3), "three rows renamed the control");
    }

    /// Not over one, where it would be a second control for what the row
    /// beneath it already does.
    #[test]
    fn the_set_is_summed_up_only_over_more_than_one() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        let mut panels = Panels::default();
        let alone = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        filters.add(Filter::Faction { id: 2, name: "Federation".into() });
        let both = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(
            !alone.iter().any(|line| line.contains("filters")),
            "{alone:?}"
        );
        assert!(both.contains(&"2 filters".to_owned()), "{both:?}");
    }

    /// A route between the systems at `addresses`
    fn a_route(addresses: &[i64]) -> Filter {
        Filter::Route {
            label: format!("A -> B{}", addresses.len()),
            systems: addresses.to_vec(),
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        }
    }

    /// The routes are counted apart from the rest of the filters
    ///
    /// A route is a line drawn across the map and a faction is a way of
    /// reading the sky it is drawn over, so a single count over both said
    /// "3 filters" about a heap of two different things.
    #[test]
    fn the_routes_are_counted_apart_from_the_filters() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        filters.add(Filter::Faction { id: 2, name: "Federation".into() });
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"2 filters".to_owned()), "{said:?}");
        assert!(said.contains(&"2 routes".to_owned()), "{said:?}");
        assert!(!said.contains(&"4 filters".to_owned()), "{said:?}");
    }

    /// One route on its own is not counted, as one filter is not
    ///
    /// Its own row already says everything a count of one could, and the
    /// control over it would do what that row's own does.
    #[test]
    fn a_single_route_is_left_to_its_own_row() {
        let mut filters = Filters::default();
        filters.add(a_route(&[1, 2]));
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(!said.iter().any(|line| line.contains("route")), "{said:?}");
    }

    /// Only a route with a search left to ask offers to be asked again
    ///
    /// Two rules in one mark. A route is the one filter that was *searched*
    /// for, so it is the only one there is anything to ask over — and a route
    /// that landed is the answer to its own question, so the mark is not
    /// drawn beside it either. What is left is the case it exists for: a leg
    /// still searching, one stopped, and one with no route to be found.
    #[test]
    fn only_a_route_left_unfound_offers_to_be_plotted_again() {
        let mut panels = Panels::default();
        let mut drawn = |filters: &mut Filters| {
            words(|ui| {
                applied(ui, filters, &mut panels, &mut 0);
            })
        };

        let mut alone = Filters::default();
        alone.add(Filter::Faction { id: 1, name: "Empire".into() });
        let said = drawn(&mut alone);
        assert!(
            !said.contains(&AGAIN.to_owned()),
            "a faction was offered a search: {said:?}",
        );

        // A route that came back. Nothing to ask again: it is the answer.
        let mut landed = Filters::default();
        landed.searching(0, a_route(&[1, 9]));
        landed.landed(a_route(&[1, 4, 9]), std::time::Duration::ZERO);
        let said = drawn(&mut landed);
        assert!(
            !said.contains(&AGAIN.to_owned()),
            "a route that landed was offered plotting over: {said:?}",
        );

        // And one that did not, whichever way it ended.
        for how in [
            crate::map::filter::Plotted::Searching,
            crate::map::filter::Plotted::Stopped,
            crate::map::filter::Plotted::Unreachable,
        ] {
            let mut unfound = Filters::default();
            unfound.searching(0, a_route(&[1, 9]));
            if how != crate::map::filter::Plotted::Searching {
                unfound.gave_up(&a_route(&[1, 9]), how);
            }
            let said = drawn(&mut unfound);
            assert!(
                said.contains(&AGAIN.to_owned()),
                "{how:?} could not be asked again: {said:?}",
            );
        }
    }

    /// A section with nothing in it says nothing
    ///
    /// Routes alone are routes alone, with no empty count for the filters
    /// standing over them.
    #[test]
    fn a_section_with_nothing_in_it_is_not_drawn() {
        let mut filters = Filters::default();
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"2 routes".to_owned()), "{said:?}");
        assert!(!said.iter().any(|line| line.contains("filters")), "{said:?}");
    }

    /// The routes stand under the rest, whatever order they were asked in
    ///
    /// The sky before what is drawn over it. A route plotted between two
    /// factions being asked for would otherwise sit up among them.
    #[test]
    fn the_routes_stand_under_the_rest() {
        let mut filters = Filters::default();
        filters.add(a_route(&[1, 2]));
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        let faction = said.iter().position(|line| line == "Empire");
        let route = said.iter().position(|line| line.contains(ARROW));
        assert!(faction < route, "{said:?}");
    }

    /// The two counts each answer for their own section
    ///
    /// Turning the routes off is no reason to turn the factions off with
    /// them, which is the whole point of the two rows being two rows.
    #[test]
    fn a_section_count_turns_off_its_own_section() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        filters.add(Filter::Faction { id: 2, name: "Federation".into() });
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));

        filters.toggle_all(&Section::Routes.rows(&filters));

        assert!(Section::Filters.on(&filters), "the filters went off too");
        assert!(!Section::Routes.on(&filters), "the routes are still asked");
    }

    /// And takes away its own section
    #[test]
    fn a_section_count_takes_away_its_own_section() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));

        filters.clear(&Section::Routes.rows(&filters));

        assert_eq!(filters.iter().count(), 1);
        assert!(Section::Routes.rows(&filters).is_empty());
        assert_eq!(
            filters.get(0).map(|held| held.filter.name()),
            Some("Empire")
        );
    }

    /// A leg of a trip, from `from` to `to`, under the trip called `trip`
    fn leg(from: &str, to: &str, trip: Option<&str>) -> Filter {
        leg_at(from, to, trip, "10")
    }

    /// The same, plotted for a ship reaching `range`
    fn leg_at(from: &str, to: &str, trip: Option<&str>, range: &str) -> Filter {
        Filter::Route {
            label: format!("{from}{ARROW}{to}"),
            systems: vec![1, 2],
            range: range.to_owned(),
            trip: trip.map(|trip| trip.to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        }
    }

    /// The section the trip through `stops`, plotted at `range`, stands in
    fn trip_at(stops: &str, range: &str) -> Section {
        Section::Trip {
            stops: stops.to_owned(),
            range: range.to_owned(),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        }
    }

    /// The legs of a trip are grouped under a row of their own
    ///
    /// A trip through several systems is several routes, and they read as the
    /// one trip they were asked for rather than as a heap of routes. A route
    /// asked for on its own belongs to no trip and stands among the loose
    /// ones.
    #[test]
    fn the_legs_of_a_trip_are_grouped_under_it() {
        let mut filters = Filters::default();
        filters.add(leg("SOL", "LAVE", Some("SOL -> LAVE -> DISO")));
        filters.add(leg("LAVE", "DISO", Some("SOL -> LAVE -> DISO")));
        filters.add(leg("WOLF 359", "SIRIUS", None));
        filters.add(Filter::Faction { id: 7, name: "Faction".to_owned() });

        let trips = Section::trips(&filters);
        let held = |section: &Section| section.rows(&filters).len();

        assert_eq!(trips, vec![trip_at("SOL -> LAVE -> DISO", "10")]);
        assert_eq!(held(&trips[0]), 2, "the trip's two legs");
        assert_eq!(held(&Section::Filters), 1, "the faction");
        assert_eq!(held(&Section::Routes), 3, "every route, legs and all");
        assert_eq!(held(&Section::Loose), 1, "the route on its own");
    }

    /// And the same stops plotted for another ship are a second trip
    ///
    /// The reported trouble. Two trips asked for back to back through the
    /// same stops, the second at another jump range, and the second's legs
    /// were taken up under the first's row: one "2 Leg Route" standing over
    /// four legs, framing that took both trips at once, and a panel that read
    /// one trip's systems at the other's range. They are two answers to two
    /// questions and are what the user asked for two of.
    #[test]
    fn the_same_stops_at_another_range_are_a_second_trip() {
        let stops = "SOL -> LAVE -> DISO";
        let mut filters = Filters::default();
        for range in ["10", "20"] {
            filters.add(leg_at("SOL", "LAVE", Some(stops), range));
            filters.add(leg_at("LAVE", "DISO", Some(stops), range));
        }

        let trips = Section::trips(&filters);
        let held = |section: &Section| section.rows(&filters).len();

        assert_eq!(trips, vec![trip_at(stops, "10"), trip_at(stops, "20")]);
        assert_eq!(held(&trips[0]), 2, "the first trip's own two legs");
        assert_eq!(held(&trips[1]), 2, "the second trip's own two legs");
    }

    /// A trip is described as the one route it is flown as
    ///
    /// Every system it passes through, in order, with the seams closed: the
    /// stop a leg lands on is the stop the next sets out from and stands in
    /// the list once, or the panel would count a jump from a system to
    /// itself.
    #[test]
    fn a_trip_is_described_as_one_route() {
        let trip = "SOL -> LAVE -> DISO";
        let hops = |from: i64, to: i64| Filter::Route {
            label: "leg".to_owned(),
            systems: vec![from, from + 1, to],
            range: "10".to_owned(),
            trip: Some(trip.to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let mut filters = Filters::default();
        filters.add(hops(1, 3));
        filters.add(hops(3, 5));
        let rows: Vec<usize> = (0..2).collect();

        let whole = as_one(trip, &rows, &filters).expect("a trip");

        assert_eq!(whole.name(), "2 Leg Route");
        assert_eq!(whole.range(), Some("10"));
        // 3 stands once, being where the first leg landed and the second set
        // out from.
        let Filter::Route { systems, .. } = &whole else { panic!("a route") };
        assert_eq!(systems, &vec![1, 2, 3, 4, 5]);
    }

    /// A trip with nothing landed yet is not described
    ///
    /// A panel about it would be a panel about no systems, and its row
    /// already says how far along the legs are.
    #[test]
    fn a_trip_with_no_legs_yet_is_not_described() {
        let filters = Filters::default();

        assert_eq!(as_one("SOL -> LAVE", &[], &filters), None);
    }

    /// A trip's legs are drawn in from its row
    ///
    /// So the block reads as one trip with its legs under it rather than as a
    /// row and then some routes. A route belonging to no trip is drawn in
    /// from nothing, standing level with the section rows.
    #[test]
    fn a_trips_legs_are_indented_under_it() {
        let trip = "SOL -> LAVE -> DISO";
        let mut filters = Filters::default();
        filters.add(leg("SOL", "LAVE", Some(trip)));
        filters.add(leg("LAVE", "DISO", Some(trip)));
        filters.add(leg("WOLF 359", "SIRIUS", None));
        let mut panels = Panels::default();

        let ctx = crate::testing::context();
        let mut drawn = |filters: &mut Filters| {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                applied(ui, filters, &mut panels, &mut 0);
            });
            let mut lefts = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    lefts.push((text.galley.text().to_owned(), text.pos.x));
                }
            }
            lefts
        };

        // Twice, the first pass being where the rows are placed.
        drawn(&mut filters);
        let lefts = drawn(&mut filters);
        let left_of = |name: &str| {
            lefts
                .iter()
                .find(|(said, _)| said == name)
                .unwrap_or_else(|| panic!("{name} was painted: {lefts:?}"))
                .1
        };

        let under = left_of("2 Leg Route");
        assert!(left_of("SOL -> LAVE") > under, "{lefts:?}");
        assert!(left_of("LAVE -> DISO") > under, "{lefts:?}");
        // The loose route belongs to no trip and is drawn in from nothing.
        assert_eq!(left_of("WOLF 359 -> SIRIUS"), under, "{lefts:?}");
    }

    /// And the routes read in the order they were plotted
    ///
    /// A trip stands where it was asked for, not ahead of everything else: a
    /// route plotted before it reads above it and one plotted after reads
    /// below. The bar drew every trip first and the loose routes after, so a
    /// route plotted first dropped to the bottom of the bar as soon as a trip
    /// landed.
    #[test]
    fn the_routes_read_in_the_order_they_were_plotted() {
        let trip = "SOL -> LAVE -> DISO";
        let mut filters = Filters::default();
        filters.add(leg("WOLF 359", "SIRIUS", None));
        filters.add(leg("SOL", "LAVE", Some(trip)));
        filters.add(leg("LAVE", "DISO", Some(trip)));
        filters.add(leg("ACHENAR", "REORTE", None));
        let mut panels = Panels::default();

        let ctx = crate::testing::context();
        let mut drawn = |filters: &mut Filters| {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                applied(ui, filters, &mut panels, &mut 0);
            });
            let mut said = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    said.push((text.galley.text().to_owned(), text.pos.y));
                }
            }
            said
        };

        // Twice, the first pass being where the rows are placed.
        drawn(&mut filters);
        let said = drawn(&mut filters);
        let top_of = |name: &str| {
            said.iter()
                .find(|(drew, _)| drew == name)
                .unwrap_or_else(|| panic!("{name} was painted: {said:?}"))
                .1
        };

        // The count over all three of them, then each where it was asked for.
        let order = [
            "3 routes",
            "WOLF 359 -> SIRIUS",
            "2 Leg Route",
            "SOL -> LAVE",
            "LAVE -> DISO",
            "ACHENAR -> REORTE",
        ];
        for pair in order.windows(2) {
            assert!(
                top_of(pair[0]) < top_of(pair[1]),
                "{} stands above {}: {said:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// And a second plot of the same trip gets a row of its own in the bar
    ///
    /// What the trouble looked like: two trips plotted back to back through
    /// the same stops, the second for a ship reaching further, and the bar
    /// drew one row over four legs. Two plots are two answers to compare, so
    /// there are two rows to turn off, close and frame apart from each other.
    #[test]
    fn a_second_plot_of_a_trip_gets_its_own_row() {
        let trip = "SOL -> LAVE -> DISO";
        let mut filters = Filters::default();
        for range in ["10", "20"] {
            filters.add(leg_at("SOL", "LAVE", Some(trip), range));
            filters.add(leg_at("LAVE", "DISO", Some(trip), range));
        }
        let mut panels = Panels::default();

        let ctx = crate::testing::context();
        let mut drawn = |filters: &mut Filters| {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                applied(ui, filters, &mut panels, &mut 0);
            });
            let mut said = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    said.push(text.galley.text().to_owned());
                }
            }
            said
        };

        // Twice, the first pass being where the rows are placed.
        drawn(&mut filters);
        let said = drawn(&mut filters);

        let rows =
            |name: &str| said.iter().filter(|drew| *drew == name).count();
        assert_eq!(rows("2 Leg Route"), 2, "one row per plot: {said:?}");
        assert_eq!(rows("SOL -> LAVE"), 2, "{said:?}");
        assert_eq!(rows("LAVE -> DISO"), 2, "{said:?}");
    }

    /// The count over the routes counts the trips as well
    ///
    /// A trip is one route flown in several legs, so two trips and two routes
    /// asked for on their own are four routes and the row says four. Counting
    /// only the loose ones left a bar of nothing but trips with no count at
    /// all, and nothing to turn every line off with.
    #[test]
    fn the_routes_count_stands_over_the_trips_too() {
        let trip = "SOL -> LAVE -> DISO";
        let mut filters = Filters::default();
        for range in ["10", "20"] {
            filters.add(leg_at("SOL", "LAVE", Some(trip), range));
            filters.add(leg_at("LAVE", "DISO", Some(trip), range));
        }
        filters.add(leg("WOLF 359", "SIRIUS", None));
        filters.add(leg("ACHENAR", "REORTE", None));
        let mut panels = Panels::default();

        let ctx = crate::testing::context();
        let mut drawn = |filters: &mut Filters| {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                applied(ui, filters, &mut panels, &mut 0);
            });
            let mut said = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    said.push((text.galley.text().to_owned(), text.pos.y));
                }
            }
            said
        };

        // Twice, the first pass being where the rows are placed.
        drawn(&mut filters);
        let said = drawn(&mut filters);
        let top_of = |name: &str| {
            said.iter()
                .find(|(drew, _)| drew == name)
                .unwrap_or_else(|| panic!("{name} was painted: {said:?}"))
                .1
        };

        // Two trips and the two loose routes, and the one count over them.
        assert!(top_of("4 routes") < top_of("2 Leg Route"), "{said:?}");
        assert!(
            !said.iter().any(|(drew, _)| drew == "2 routes"),
            "the loose routes are counted with the trips: {said:?}"
        );
    }

    /// And its gestures reach the legs of those trips
    ///
    /// The row says four routes, so closing it closes four routes: a count
    /// that stood over the trips and let go of only the loose ones would be
    /// saying one thing and doing another. What is no route is left alone.
    #[test]
    fn the_routes_count_lets_go_of_the_trips_too() {
        let trip = "SOL -> LAVE -> DISO";
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 7, name: "Empire".to_owned() });
        filters.add(leg("SOL", "LAVE", Some(trip)));
        filters.add(leg("LAVE", "DISO", Some(trip)));
        filters.add(leg("WOLF 359", "SIRIUS", None));

        filters.clear(&Section::Routes.rows(&filters));

        let held: Vec<&str> =
            filters.iter().map(|active| active.filter.name()).collect();
        assert_eq!(held, vec!["Empire"]);
    }

    /// A press on a trip's row picks out every line of it
    ///
    /// A trip is one thing the user is working with, so the whole of it
    /// stands in front: its stops are picked out on the map and its legs are
    /// the routes drawn at full strength. Handing back only the stops left
    /// the lines as they were, and a trip pressed had whichever route was
    /// plotted last drawn in front of it.
    #[test]
    fn a_trips_row_picks_out_every_line_of_it() {
        let trip = "SOL -> LAVE -> DISO";
        let legs =
            [leg("SOL", "LAVE", Some(trip)), leg("LAVE", "DISO", Some(trip))];
        let mut filters = Filters::default();
        for leg in &legs {
            filters.add(leg.clone());
        }
        let mut panels = Panels::default();

        let ctx = crate::testing::context();
        let mut pass = |filters: &mut Filters, input| {
            let mut asked = RowAsk::default();
            let _ = ctx.run_ui(input, |ui| {
                asked = applied(ui, filters, &mut panels, &mut 0);
            });
            asked
        };
        // Over the trip's own row, which is the first drawn: one trip and no
        // route beside it is nothing for a count to stand over.
        let at = egui::pos2(60., 13.);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };

        // The first pass places the rows, the second presses, and the third
        // is past the window a double click could still arrive in.
        pass(&mut filters, egui::RawInput::default());
        pass(
            &mut filters,
            egui::RawInput {
                time: Some(1.),
                events: vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
                ..Default::default()
            },
        );
        let asked = pass(
            &mut filters,
            egui::RawInput { time: Some(1.4), ..Default::default() },
        );

        assert_eq!(asked.chosen, legs.to_vec(), "every leg of it");
        assert!(asked.picked.is_some(), "and its stops: {:?}", asked.picked);
    }

    /// A trip's row says what the whole of it is flown in
    ///
    /// The figure a trip was plotted to find out, where each of its legs says
    /// its own: how many jumps it takes altogether. Read off the legs held,
    /// so it is the sum of them rather than a number of its own to keep in
    /// step.
    #[test]
    fn a_trips_row_says_the_jumps_of_the_whole_trip() {
        let trip = "SOL -> LAVE -> DISO";
        let flown = |from: &str, to: &str, hops: usize| Filter::Route {
            label: format!("{from}{ARROW}{to}"),
            // One more system than jumps: a route of two systems is one jump.
            systems: (0..=hops as i64).collect(),
            range: "10".to_owned(),
            trip: Some(trip.to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let mut filters = Filters::default();
        filters.add(flown("SOL", "LAVE", 3));
        filters.add(flown("LAVE", "DISO", 5));
        let mut panels = Panels::default();

        let ctx = crate::testing::context();
        let mut drawn = |filters: &mut Filters| {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                applied(ui, filters, &mut panels, &mut 0);
            });
            let mut said = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    said.push((text.galley.text().to_owned(), text.pos.y));
                }
            }
            said
        };

        // Twice, the first pass being where the rows are placed.
        drawn(&mut filters);
        let said = drawn(&mut filters);
        let top_of = |name: &str| {
            said.iter()
                .find(|(drew, _)| drew == name)
                .unwrap_or_else(|| panic!("{name} was painted: {said:?}"))
                .1
        };

        // Three and five, said where the row that stands over both is.
        assert_eq!(top_of("8 hops"), top_of("2 Leg Route"), "{said:?}");
        assert!(top_of("3 hops") > top_of("8 hops"), "{said:?}");
        assert!(top_of("5 hops") > top_of("3 hops"), "{said:?}");
    }

    /// And a trip stopped part way keeps a row for every leg of it
    ///
    /// The reported trouble: a trip of three legs cancelled before the last
    /// one landed drew a **"3 Leg Route" over two rows**, with nothing to say
    /// where the third went. Its row goes up when the leg is asked for, so it
    /// stands and says it was stopped — and stays out of what the trip is
    /// flown in, that being what has actually been found.
    #[test]
    fn a_stopped_leg_keeps_its_row_and_stays_out_of_the_total() {
        let stops = ["SOL", "LAVE", "DISO", "REORTE"];
        let trip = stops.join(ARROW);
        // A leg's two ends are fixed and what it flies between them is the
        // answer: the ask carries the ends alone, which is how the row is
        // found again when the answer lands.
        let leg = |at: usize, hops: i64| {
            let start = 100 * at as i64;
            let mut systems = vec![start];
            systems.extend((1..hops).map(|k| start + k));
            systems.push(start + 99);
            Filter::Route {
                label: format!("{}{ARROW}{}", stops[at], stops[at + 1]),
                systems,
                range: "10".to_owned(),
                trip: Some(trip.clone()),
                drive: Drive::Unaided,
                how: Routing::default(),
                tune: Tuning::default(),
            }
        };

        let mut filters = Filters::default();
        for at in 0..3 {
            let asked = leg(at, 1);
            let place = crate::map::route::placed_at(&asked, &filters);
            filters.searching(place, asked);
        }
        // Two of them land; the third is still searching when the plot is
        // stopped.
        filters.landed(leg(0, 3), std::time::Duration::ZERO);
        filters.landed(leg(1, 5), std::time::Duration::ZERO);
        filters.stopped_searching();

        assert_eq!(filters.iter().count(), 3, "the stopped leg lost its row");

        let mut panels = Panels::default();
        let ctx = crate::testing::context();
        let mut drawn = |filters: &mut Filters| {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                applied(ui, filters, &mut panels, &mut 0);
            });
            let mut said = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    said.push(text.galley.text().to_owned());
                }
            }
            said
        };
        drawn(&mut filters);
        let said = drawn(&mut filters);

        assert!(said.contains(&"3 Leg Route".to_owned()), "{said:?}");
        assert!(said.contains(&"DISO -> REORTE".to_owned()), "{said:?}");
        assert!(said.contains(&"stopped".to_owned()), "{said:?}");
        // Three and five, and nothing claimed for the leg nobody flew.
        assert!(said.contains(&"8 hops".to_owned()), "{said:?}");

        // And the trip as one route is the legs that landed.
        let section = Section::of(&leg(0, 3)).expect("a leg of a trip");
        let rows = section.rows(&filters);
        let whole = as_one(&trip, &rows, &filters).expect("a joined trip");
        assert_eq!(whole.hops(), Some(8), "the stopped leg was counted in");
    }

    /// And a trip's row offers to be asked again only while a leg is unfound
    ///
    /// One route flown in several searches: while any of them is still to be
    /// found the whole trip is worth asking over, and once every leg has
    /// landed there is nothing to ask — the trip is the answer to its own
    /// question, and a mark offering to put it again would put the same
    /// question twice.
    #[test]
    fn a_trips_row_offers_a_replot_only_while_a_leg_is_unfound() {
        let stops = ["SOL", "LAVE", "DISO"];
        let trip = stops.join(ARROW);
        let leg = |at: usize, hops: i64| {
            let start = 100 * at as i64;
            let mut systems = vec![start];
            systems.extend((1..hops).map(|k| start + k));
            systems.push(start + 99);
            Filter::Route {
                label: format!("{}{ARROW}{}", stops[at], stops[at + 1]),
                systems,
                range: "10".to_owned(),
                trip: Some(trip.clone()),
                drive: Drive::Unaided,
                how: Routing::default(),
                tune: Tuning::default(),
            }
        };

        let mut filters = Filters::default();
        for at in 0..2 {
            let asked = leg(at, 1);
            let place = crate::map::route::placed_at(&asked, &filters);
            filters.searching(place, asked);
        }
        let section = Section::of(&leg(0, 1)).expect("a leg of a trip");

        // One leg landed, the other still out: the trip has a search left.
        filters.landed(leg(0, 3), std::time::Duration::ZERO);
        assert!(section.unfound(&filters), "a leg was still being searched");

        // And with the last of them in, nothing to ask.
        filters.landed(leg(1, 5), std::time::Duration::ZERO);
        assert!(
            !section.unfound(&filters),
            "a trip that landed whole was offered plotting over",
        );

        // Which is what the row draws: the mark, and then no mark.
        let mut panels = Panels::default();
        let mut drawn = |filters: &mut Filters| {
            words(|ui| {
                applied(ui, filters, &mut panels, &mut 0);
            })
        };
        let whole = drawn(&mut filters);
        assert!(!whole.contains(&AGAIN.to_owned()), "{whole:?}");

        // And it comes back the moment a leg is being searched again.
        filters.searching(1, leg(1, 1));
        let waiting = drawn(&mut filters);
        assert!(waiting.contains(&AGAIN.to_owned()), "{waiting:?}");
    }

    /// A trip's row is named for how many legs it is
    ///
    /// Not for its stops: they are the rows under it, named there and in that
    /// order, and a trip through six of them spelled out runs longer than the
    /// bar is wide. What the whole of it comes to is the panel's to say.
    #[test]
    fn a_trips_row_is_named_for_its_legs() {
        assert_eq!(trip_at("SOL -> LAVE", "10").said(1), "1 Leg Route");
        assert_eq!(trip_at("SOL -> LAVE -> DISO", "10").said(2), "2 Leg Route");
    }

    /// And a trip with nothing landed yet claims no jumps at all
    ///
    /// Its rows go up when its legs are asked for, so the count over them is
    /// drawn before any of them has an answer: a nought there reads as a trip
    /// that came back with nothing, where what is true is that nothing has
    /// come back yet.
    #[test]
    fn a_trip_still_searching_says_no_jumps_rather_than_none_flown() {
        let trip = "SOL -> LAVE -> DISO";
        let leg = |from: &str, to: &str| Filter::Route {
            label: format!("{from}{ARROW}{to}"),
            systems: vec![1, 9],
            range: "10".to_owned(),
            trip: Some(trip.to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let mut filters = Filters::default();
        filters.searching(0, leg("SOL", "LAVE"));
        let section = Section::of(&leg("SOL", "LAVE")).expect("a trip's leg");

        assert_eq!(section.hops(&filters), None, "a nought was claimed");

        // And the moment one lands, what it came to is said.
        let mut flown = leg("SOL", "LAVE");
        if let Filter::Route { systems, .. } = &mut flown {
            *systems = vec![1, 4, 9];
        }
        filters.landed(flown, std::time::Duration::ZERO);

        assert_eq!(section.hops(&filters), Some(2));
    }

    /// A section's own row reads the same way as the rows under it
    ///
    /// The dot turns all of them off, a double click frames all of them, and
    /// the mark takes them all away -- the row's three gestures said of the
    /// whole section at once, which is what the row is for.
    ///
    /// A click on the name asks nothing. There is no one filter for a section
    /// to be the one being worked with, so the gesture that would say which
    /// has nothing to say here.
    #[test]
    fn a_sections_row_reads_like_the_rows_under_it() {
        let asked = |close, switch, double| match asked_of_row(
            close, false, false, switch, double, false,
        ) {
            Some(RowGesture::LetGo) => Some(FilterAction::LetGo),
            Some(RowGesture::Toggle) => Some(FilterAction::Toggle),
            Some(RowGesture::Frame) => Some(FilterAction::Frame),
            _ => None,
        };

        assert_eq!(asked(false, true, false), Some(FilterAction::Toggle));
        assert_eq!(asked(false, false, true), Some(FilterAction::Frame));
        assert_eq!(asked(true, false, false), Some(FilterAction::LetGo));
        // A press on the name alone, which used to turn them all off.
        assert_eq!(asked(false, false, false), None);
    }

    #[test]
    fn a_route_row_says_how_many_jumps_it_is() {
        let mut filters = Filters::default();
        filters.add(Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2, 3, 4, 5],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        });
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"4 hops".to_owned()), "{said:?}");
    }

    /// A route row too narrow for its name keeps both ends of it
    ///
    /// The row cuts what it draws to what the dot, the count and the marks
    /// leave, and a route cut from the right hand end is a route that no
    /// longer says where it goes.
    #[test]
    fn a_route_row_cut_down_still_says_where_it_goes() {
        let mut filters = Filters::default();
        filters.add(Filter::Route {
            label: "SIGMA DRACONIS -> MINISTRY".to_owned(),
            systems: vec![1, 2, 3],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        });
        let mut panels = Panels::default();

        let said = words(|ui| {
            // Too narrow for the name, whatever the bar is set to.
            ui.set_max_width(200.);
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        let name = said
            .iter()
            .find(|line| line.contains(ARROW))
            .unwrap_or_else(|| panic!("{said:?}"));
        let (from, to) = name.split_once(ARROW).expect("a route");
        // Something gave, and what is left of either end is that end: the
        // start of the name it was cut from, rather than a stretch of the
        // other one that happened to be nearer the middle.
        assert!(name.contains(CUT), "{name:?}");
        let (from, to) = (from.trim_end_matches(CUT), to.trim_end_matches(CUT));
        assert!(
            !from.is_empty() && "SIGMA DRACONIS".starts_with(from),
            "{name:?}"
        );
        assert!(!to.is_empty() && "MINISTRY".starts_with(to), "{name:?}");
    }

    /// One jump is a hop rather than one hops
    #[test]
    fn a_route_of_one_jump_says_it_in_the_singular() {
        let mut filters = Filters::default();
        filters.add(Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        });
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"1 hop".to_owned()), "{said:?}");
    }

    /// Egui says nothing about the ids the filter rows use
    #[test]
    fn the_filter_rows_do_not_share_ids() {
        use crate::testing::complaints;

        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Alpha".into() });
        filters.add(Filter::Faction { id: 2, name: "Beta".into() });
        filters.add(Filter::Route {
            label: "A -> B".into(),
            systems: vec![1, 2],
            range: "10".into(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        });
        let mut panels = Panels::default();

        let said = complaints(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.is_empty(), "{said:?}");
    }

    /// What the spyglass reaches is said with nothing asked of the map
    ///
    /// One number, since with nothing excluded the two are the same number
    /// and saying it twice says nothing. Worth saying at all because it is
    /// what tells the user whether they are looking at a handful of systems
    /// or a hundred thousand.
    #[test]
    fn the_reach_alone_is_one_number() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 324, total: 324 },
                false,
                false,
                false,
            )
        });

        assert!(said.contains(&"324 in spyglass".to_owned()), "{said:?}");
    }

    /// With something excluded and drawn faintly, both numbers are said
    ///
    /// The sky behind what is picked out is on screen, so how much of it
    /// there is answers what the user can see.
    #[test]
    fn what_is_dimmed_is_counted_behind_what_is_not() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 8, total: 324 },
                true,
                false,
                false,
            )
        });

        assert!(said.contains(&"8 of 324 in spyglass".to_owned()), "{said:?}");
    }

    /// With it not drawn at all, only what can be seen is said
    ///
    /// The excluded systems are neither on screen nor fetched, so the larger
    /// number describes a place the user cannot see and a sky the map has not
    /// got.
    #[test]
    fn what_is_not_drawn_is_not_counted() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 8, total: 324 },
                false,
                false,
                false,
            )
        });

        assert!(said.contains(&"8 in spyglass".to_owned()), "{said:?}");
        assert!(!said.iter().any(|line| line.contains("324")), "{said:?}");
    }

    /// An empty sky says nothing rather than saying it is empty
    ///
    /// Which is the map before its first fetch lands, where a nought would
    /// read as an answer rather than as nothing having been asked yet.
    #[test]
    fn an_empty_reach_says_nothing() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 0, total: 0 },
                false,
                false,
                false,
            )
        });

        assert!(!said.iter().any(|line| line.contains("spyglass")), "{said:?}");
    }

    /// And so does a sky of one, however it comes to be one
    ///
    /// Which is every descent: the camera inside a system holds that system
    /// and nothing else, and `1 in spyglass` beside the system's own name is
    /// a number saying less than the word next to it.
    #[test]
    fn a_reach_of_one_system_says_nothing() {
        for reach in [
            InReach { admitted: 1, total: 1 },
            InReach { admitted: 0, total: 1 },
        ] {
            let dimming = reach.admitted != reach.total;
            let said = words(|ui| reaching(ui, &reach, dimming, false, false));

            assert!(
                !said.iter().any(|line| line.contains("spyglass")),
                "{said:?}"
            );
        }
    }

    /// The filter rows come out in colors something can draw
    ///
    /// Every galley here is laid out strong or weak, which resolves a color,
    /// so the placeholder each is painted with is never reached. That holds
    /// by how the rows happen to be styled and nothing else, and one plain
    /// piece of text would take the whole bar down.
    ///
    /// Covers the marks as well, which the selection's row draws the same
    /// way.
    #[test]
    fn the_filter_rows_paint_in_colors() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Zargon Front".into() });
        filters.add(Filter::Faction { id: 2, name: "Alliance".into() });
        filters.toggle(1);
        let mut panels = Panels::default();

        painted(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });
    }
}
