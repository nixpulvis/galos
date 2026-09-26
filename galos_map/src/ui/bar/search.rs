//! Searching for a system by name, and the list of what came back

use crate::map::camera::MoveCamera;
use crate::map::route::frontier::Frontiers;
use crate::map::search::{Plot, Search, SearchNote, SearchResults, Searching};
use crate::map::selection::{Picked, Selection};
use crate::ui::list::{
    LINE_PADDING, Rows, SystemAction, scrolling, system_line,
};
use crate::ui::panels::Panels;
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::Ui;
use galos_index::records::NameEntry;
use galos_route::graph::{Drive, Routing, Tuning};

/// The whole of the bar's searching
///
/// What was asked, what came back, and whether an answer is late. Gathered as
/// the filters are: the bar is drawn by one system, a system may take sixteen
/// things, and the bar asks about more than sixteen.
#[derive(SystemParam)]
pub(crate) struct SearchBar<'w> {
    /// Where a name typed into a field is sent to be looked up
    pub(in crate::ui) search: MessageWriter<'w, Search>,
    /// What to say about a name that found nothing
    pub(in crate::ui) note: ResMut<'w, SearchNote>,
    /// The systems the search box found
    pub(in crate::ui) results: ResMut<'w, SearchResults>,
    /// What the search box has out
    pub(in crate::ui) pending: Res<'w, Searching>,
    /// How the route last asked for is getting on
    pub(in crate::ui) plot: ResMut<'w, Plot>,
    /// Which of the fewest-jumps routes to ask for
    pub(in crate::ui) how: ResMut<'w, Routing>,
    pub(in crate::ui) drive: ResMut<'w, Drive>,
    /// How a long supercharged route is planned; see `planning`
    pub(in crate::ui) tune: ResMut<'w, Tuning>,
    /// Whether the index publishes a supercharge table at all, which is what
    /// a route for a supercharging drive needs before it can be asked for
    pub(in crate::ui) boosts: Res<'w, galos_route::Boosts>,
    pub(in crate::ui) searching: Res<'w, Frontiers>,
}

/// Take away everything standing as an answer to the name in the box
///
/// The query, the note about a name that resolved to nothing, and the list of
/// what it might have meant. One gesture takes all three because they are one
/// answer: a list left standing under an empty box answers a question that is
/// no longer on screen to be read.
pub(super) fn cleared(
    value: &mut Option<String>,
    note: &mut SearchNote,
    results: &mut SearchResults,
) {
    *value = None;
    note.0 = None;
    results.clear();
}

/// How many of what a search found the bar shows before the list scrolls
///
/// A screenful of the bar rather than of the viewport. The list hangs under
/// the input with the map behind it, and one long enough to reach the bottom
/// of the screen would answer which system did you mean by covering over the
/// sky the answer is about.
pub(super) const OFFERED: usize = 5;

/// Act on what a line was asked for
///
/// Apart from the drawing, since a list draws every line before any of them
/// is acted on: picking one out changes what the lines are drawn from. Which
/// makes it the piece worth asking about on its own.
fn act_on(
    action: SystemAction,
    system: &NameEntry,
    at: DVec3,
    selection: &mut Selection,
    travelled: &mut Option<DVec3>,
    described: &mut Option<crate::map::galaxy::System>,
) {
    // `at` is where the galaxy says it is, asked once here rather than per
    // frame: a line is drawn every frame and acted on when it is clicked,
    // and a published row carries no place to read off. The list's own
    // ordering and distance readout stand on the boxel middle — see
    // [`crate::map::galaxy::system_to_vec`].
    //
    // Its political columns fill in when a fetch draws it.
    let placed = crate::map::galaxy::System::named_at(system, at);
    match action {
        SystemAction::Select { gathering } => {
            selection.pick(Picked::System(placed), gathering);
        }
        SystemAction::Travel => *travelled = Some(at),
        SystemAction::Describe => *described = Some(placed),
    }
}

/// The systems the last search found, for the user to choose between
///
/// Every search is answered here, whether the user typed part of a name or the
/// whole of one. A name spelled out in full leads the list rather than being
/// picked out on its own: the search says which systems are on record under
/// that name and the click says which of them is meant, and a search that
/// picked something out would let go of whatever had been gathered before it.
/// This stands where the note would, the two never appearing together, since a
/// search either found systems to list or found nothing and says so.
///
/// The list is left standing once something is picked out of it. Choosing is
/// most of what it is for, and a list that puts itself away as soon as it is
/// touched makes trying the second candidate a matter of typing the whole
/// query again.
///
/// A line answers the gestures a star answers. A plain click picks that system
/// out in place of the rest — or gathers it, where `picking` says the list is
/// being read for stops; see [`Picking`] — a click with ctrl, command or
/// shift held gathers it up alongside them and lets go of one already held,
/// and a double click sends the camera there. Gathering reaches across
/// searches: the list goes when the next name is typed and what was picked
/// out of it stays, so a set can be built a name at a time.
///
/// Every system listed can be picked. The resident table is names and
/// positions in one, so an entry is a placed system by construction and there
/// is no line here the camera cannot be sent to.
///
/// Each line carries the info mark the rows in the bar carry, opening what is
/// known about that system without picking it out. That is how a list of
/// candidates is read: several are opened and compared while the selection
/// stays wherever the user left it, which is the whole point of being handed
/// several rather than one.
///
/// `travelled` is where a line asked the camera to go and `described` is what
/// a line asked to be written out, both of which the caller acts on rather
/// than this: a message writer cannot be had outside a system, and a list that
/// reports what it was asked for can be drawn in a test.
///
/// Drawn only while the box is asking about a name, which is [`ask_bar`](crate::ui::bar::ask_bar)'s to
/// say: the search asks one, and so does the route, which runs between the
/// systems a name is asked about. One left standing under a shut form — or
/// under a box that has since been turned to asking for a faction — is an
/// answer to a question that is no longer on screen. What was found is kept,
/// so opening the form again is where they left off rather than a search to
/// do a second time.
///
/// Unlike the rows in the state bar, which stand whether or not the form is
/// out. A selection and a filter outlive the asking and go on saying what the
/// map is doing; a list of candidates is the asking itself.
#[allow(clippy::too_many_arguments)]
pub(super) fn found(
    ui: &mut Ui,
    results: &SearchResults,
    center: Option<DVec3>,
    names: &crate::map::index::Names,
    picking: Picking,
    selection: &mut Selection,
    travelled: &mut Option<DVec3>,
    described: &mut Option<crate::map::galaxy::System>,
) {
    if results.is_empty() {
        return;
    }

    let Some((system, action)) =
        system_list(ui, results.iter(), center, "result")
    else {
        return;
    };
    act_on(
        picking.of(action),
        system,
        names.placed(system.address),
        selection,
        travelled,
        described,
    );
}

/// What a plain click on a line the search found means
///
/// The gestures are the same either way — a modifier gathers, a double click
/// flies there, the mark opens what is known — and what differs is what the
/// plain click on its own does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Picking {
    /// Stand in for whatever was picked out, unless a modifier says otherwise
    ///
    /// Which is how a star on the map answers a click, and what a search is:
    /// one name asked about, and the system meant picked out.
    Asked,
    /// Gather it up alongside them, modifier or not
    ///
    /// What the route asks. A trip is two or more systems, so a click that
    /// let go of the last stop to take the next could never build one, and
    /// holding a modifier down to add the thing the form exists to add is a
    /// form that argues with itself.
    Gathers,
}

impl Picking {
    /// What a line's gesture comes to under this reading of a plain click
    fn of(self, action: SystemAction) -> SystemAction {
        match (self, action) {
            (Picking::Gathers, SystemAction::Select { .. }) => {
                SystemAction::Select { gathering: true }
            }
            (_, action) => action,
        }
    }
}

/// What came back of the name in the box: the systems found, or that none was
///
/// The two never stand together — a search either found systems to list or
/// found nothing and says so — and neither is drawn under a box asking about
/// something else. Both modes that ask about a name draw this: the search,
/// where a click picks a system out, and the route, where it adds a stop.
/// See [`Picking`].
///
/// Where a line asked the camera to go and what it asked to be written out
/// are acted on here rather than handed back, both of them being messages
/// this has the writers for.
#[allow(clippy::too_many_arguments)]
pub(super) fn answer(
    ui: &mut Ui,
    note: &SearchNote,
    results: &SearchResults,
    center: Option<DVec3>,
    names: &crate::map::index::Names,
    picking: Picking,
    selection: &mut Selection,
    panels: &mut Panels,
    camera: &mut MessageWriter<MoveCamera>,
) {
    if let Some(note) = &note.0 {
        ui.colored_label(egui::Color32::LIGHT_RED, note);
    }

    let mut travelled = None;
    let mut described = None;
    found(
        ui,
        results,
        center,
        names,
        picking,
        selection,
        &mut travelled,
        &mut described,
    );
    if let Some(position) = travelled {
        camera.write(MoveCamera { position: Some(position), framing: None });
    }
    if let Some(system) = described {
        panels.open_system(system);
    }
}

/// A list of systems, and what a click asked of one of them
///
/// Every list of systems the map offers to be chosen from is this, the search
/// results among them. What a click means is the caller's, since that is the
/// only part that differs between one list and another, and the lines
/// themselves are [`system_line`] so that a system is read the same way
/// wherever it is listed.
///
/// Scrolls past [`OFFERED`], which is a screenful of the bar rather than of
/// the viewport: the list hangs over the map and one long enough to reach the
/// bottom of the viewport answers a question by covering up what it is about.
///
/// `center` is where distances are measured from, and nothing where the camera
/// has yet to say. With nothing to measure from a line carries no distance
/// rather than one measured from somewhere else.
///
/// `salt` keys one list's lines apart from another's. Within a list they are
/// keyed by place rather than by which system a line is about, as the rows in
/// the bar are keyed and for the reason given there: a fresh search leaves the
/// lines where they were and makes every one of them about something else.
pub(crate) fn system_list<'a>(
    ui: &mut Ui,
    systems: impl Iterator<Item = &'a NameEntry>,
    center: Option<DVec3>,
    salt: &str,
) -> Option<(&'a NameEntry, SystemAction)> {
    // Nothing found is nothing drawn, rather than an empty list taking a
    // line's worth of room under the field that has yet to be asked.
    let mut systems = systems.peekable();
    systems.peek()?;

    // Gathered before any of it is drawn, since how the first line reads
    // depends on what the last one needs -- see [`Rows`]. Each line's
    // distance is said once here rather than measured once and formatted
    // again.
    let listed: Vec<(&NameEntry, Option<String>)> = systems
        .map(|system| {
            let at = crate::map::galaxy::system_to_vec(system);
            // How far off it is, where there is anywhere to measure from.
            let away =
                center.map(|center| format!("{:.1} Ly", center.distance(at)));
            (system, away)
        })
        .collect();
    let rows = Rows::of(
        ui,
        ui.available_width(),
        listed
            .iter()
            .map(|(system, away)| (system.name.as_str(), away.as_deref())),
    );

    let height = ui.text_style_height(&egui::TextStyle::Body)
        + LINE_PADDING * 2.
        + ui.spacing().item_spacing.y;

    // Settled after the list is drawn, since what a click asks for is usually
    // a change to what the lines are being drawn from.
    let mut chose = None;

    scrolling(ui, height * OFFERED as f32, salt, |ui| {
        for (index, (system, away)) in listed.into_iter().enumerate() {
            let asked =
                system_line(ui, &system.name, away, rows, (salt, index));
            if let Some(asked) = asked {
                chose = Some((system, asked));
            }
        }
    });

    chose
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::map::search::tests::row;
    use crate::testing::{painted, words};
    use crate::ui::bar::route::stops_of;
    use crate::ui::bar::{AskMode, ask_box, mode_strip};

    use crate::ui::testing::{
        clicking, draw_selected, holding, results, results_of,
    };

    use crate::ui::{
        BAR_WIDTH, CLOSE, Dropping, FIELD_GAP, INFO, MARGIN, Standing,
    };

    /// How many of `names` were painted whole, rather than clipped away
    fn shown_whole(output: &egui::FullOutput, names: &[String]) -> usize {
        fn seen(
            clip: egui::Rect,
            shape: &egui::Shape,
            into: &mut Vec<(String, egui::Rect, egui::Rect)>,
        ) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    text.galley.text().into(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                    clip,
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        seen(clip, shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut painted = Vec::new();
        for clipped in &output.shapes {
            seen(clipped.clip_rect, &clipped.shape, &mut painted);
        }
        painted
            .iter()
            .filter(|(said, at, clip)| {
                names.iter().any(|name| name == said) && clip.contains_rect(*at)
            })
            .count()
    }

    /// A list that comes back under a bar which had been shut stands its
    /// whole height
    ///
    /// Reported: the search's results came back three lines tall rather than
    /// five once the form had been put away and opened again. The route's
    /// form was right, and stayed right when the box was turned from the
    /// route to the search, so it showed in the search alone.
    ///
    /// Egui lays an area's contents out in the rectangle the pass before left
    /// behind, and a scroll area holds itself to the height it is offered. So
    /// a list coming back under a bar that had been shut was offered the shut
    /// bar's height, took three lines of it, and the bar then came out three
    /// lines tall — which is the same short offer next pass, for good. The
    /// route's form stands under the list and is tall enough that the offer
    /// was never short, which is why the route never showed it.
    #[test]
    fn a_list_that_comes_back_stands_its_whole_height() {
        let names: Vec<String> = (0..25).map(|n| format!("COL {n}")).collect();
        let listed: Vec<&str> = names.iter().map(String::as_str).collect();
        let offers = results_of(&listed);

        let ctx = crate::testing::context();
        let mut selection = Selection::default();
        let mut shown = 0;
        // Out, away, and out again. Two passes apiece, so that what egui kept
        // of the pass before is a pass in the same state.
        for out in [true, true, false, false, true, true] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0., 0.),
                    egui::vec2(1440., 900.),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let ctx = ui.ctx().clone();
                Dropping {
                    id: "main-bar",
                    standing: Standing::At(egui::pos2(MARGIN, MARGIN)),
                    out,
                    width: BAR_WIDTH,
                    holds_width: true,
                }
                .show(&ctx, |ui| {
                    let mut query = Some("COL".to_owned());
                    ask_box(ui, &mut query, "Search", true, false);
                    if !out {
                        return;
                    }
                    ui.add_space(FIELD_GAP);
                    mode_strip(ui, &mut AskMode::System);
                    found(
                        ui,
                        &offers,
                        None,
                        &crate::map::index::Names::default(),
                        Picking::Asked,
                        &mut selection,
                        &mut None,
                        &mut None,
                    );
                });
            });
            shown = shown_whole(&output, &names);
        }

        assert_eq!(
            shown, OFFERED,
            "the list came back {shown} lines tall of the {OFFERED} it holds"
        );
    }

    /// What the list says, drawn from `filter`
    fn listed(results: &SearchResults, center: Option<DVec3>) -> Vec<String> {
        words(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                results,
                center,
                &crate::map::index::Names::default(),
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        })
    }

    /// Every system found is named, whatever it is
    #[test]
    fn the_list_names_what_was_found() {
        let said = listed(&results(&["SOL", "SOLATI"], true), None);

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(said.contains(&"SOLATI".to_owned()), "{said:?}");
    }

    /// And says how far off it is, from where the camera is looking
    ///
    /// Said as well as sorted by. A list in an order nobody can see reads as
    /// an order nobody chose.
    #[test]
    fn the_list_says_how_far_off_each_system_is() {
        let said =
            listed(&results(&["SOL"], true), Some(DVec3::new(3., 4., 0.)));

        assert!(said.contains(&"5.0 Ly".to_owned()), "{said:?}");
    }

    /// With no camera there is no distance to give
    #[test]
    fn a_list_measured_from_nowhere_gives_no_distance() {
        let said = listed(&results(&["SOL"], true), None);

        assert!(!said.iter().any(|line| line.contains("Ly")), "{said:?}");
    }

    /// Every system that can be had carries the mark that describes it
    ///
    /// Which is how a list of candidates is read through: several are opened
    /// and compared while the selection stays where the user left it.
    #[test]
    fn each_result_carries_a_mark_that_describes_it() {
        let said = listed(&results(&["SOL", "SOLATI"], true), None);

        assert_eq!(said.iter().filter(|line| *line == INFO).count(), 2);
    }

    /// A click on the line for `name`, with or without the modifier held
    ///
    /// The camera and the panel are not what these are about, so what a line
    /// asks of them is taken and dropped.
    fn clicked(gathering: bool, name: &str, selection: &mut Selection) {
        picked(gathering, &row(name), selection);
    }

    /// A click on the line for `system`, whatever the row says about it
    fn picked(gathering: bool, system: &NameEntry, selection: &mut Selection) {
        let mut travelled = None;
        let mut described = None;
        act_on(
            SystemAction::Select { gathering },
            system,
            crate::map::galaxy::system_to_vec(system),
            selection,
            &mut travelled,
            &mut described,
        );
    }

    /// A plain click picks one out in place of whatever was held
    ///
    /// As clicking a star does. The list is where a search is answered from,
    /// so the two gestures have to mean the same thing.
    #[test]
    fn a_click_on_a_line_replaces_what_is_held() {
        let mut selection = Selection::default();

        clicked(false, "SOL", &mut selection);
        clicked(false, "SOLATI", &mut selection);

        assert_eq!(selection.len(), 1);
        assert_eq!(selection.get(0).map(Picked::name), Some("SOLATI"));
    }

    /// A click with a modifier held gathers them up instead
    ///
    /// Several candidates come back from one search, and picking a handful of
    /// them out is what the list is for. Made to mean the same in the list as
    /// it does in the sky, since a user who has learnt the gesture on a star
    /// has learnt it.
    #[test]
    fn a_gathered_click_holds_what_was_already_picked() {
        let mut selection = Selection::default();

        clicked(false, "SOL", &mut selection);
        clicked(true, "SOLATI", &mut selection);

        assert_eq!(selection.get(0).map(Picked::name), Some("SOL"));
        assert_eq!(selection.get(1).map(Picked::name), Some("SOLATI"));
    }

    /// And one already held is let go of
    ///
    /// One gesture that builds a set and takes it apart, as it is on the map.
    #[test]
    fn a_gathered_click_on_one_already_held_lets_go_of_it() {
        let mut selection = Selection::default();

        clicked(false, "SOL", &mut selection);
        clicked(true, "SOLATI", &mut selection);
        clicked(true, "SOL", &mut selection);

        assert_eq!(selection.len(), 1);
        assert_eq!(selection.get(0).map(Picked::name), Some("SOLATI"));
    }

    // Only the debug-only passes below use it: egui compiles its
    // between-pass id check out of a release build. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    /// Draw the results list holding `names`
    fn draw_found<'a>(names: &'a [&'a str]) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &results(names, true),
                None,
                &crate::map::index::Names::default(),
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        }
    }

    /// A list whose items change is not read as a widget changing identity
    ///
    /// Egui watches for a rect that keeps its place while everything in it
    /// changes id, and paints a red rectangle over it as well as warning. A
    /// fresh search and a replaced selection are both exactly that shape, so
    /// the rows are keyed on where they sit and the ids stay put while what
    /// they are about changes underneath.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn a_list_whose_items_change_is_not_an_id_change() {
        let results = crate::testing::between_passes(
            draw_found(&["SOL", "SOLATI"]),
            draw_found(&["BARNARD", "WOLF 359"]),
        );
        let replaced = crate::testing::between_passes(
            draw_selected(&["SOL"]),
            draw_selected(&["BARNARD"]),
        );
        let shorter = crate::testing::between_passes(
            draw_selected(&["SOL", "BARNARD", "WOLF 359"]),
            draw_selected(&["SOL", "WOLF 359"]),
        );

        assert!(results.is_empty(), "{results:?}");
        assert!(replaced.is_empty(), "{replaced:?}");
        assert!(shorter.is_empty(), "{shorter:?}");
    }

    /// What the bar's box says, holding `query` against `results`
    fn box_said(query: Option<&str>, results: &[&str]) -> Vec<String> {
        words(|ui| {
            let mut value = query.map(str::to_owned);
            let results = results_of(results);
            ask_box(ui, &mut value, "Search", !results.is_empty(), false);
        })
    }

    /// What is picked out after a line naming `name` is clicked
    ///
    /// Three passes: egui knows where a widget stands only once it has been
    /// drawn, so the click lands on the pass after the list was laid out.
    fn line_clicked(picking: Picking, held: &[&str], name: &str) -> Selection {
        let ctx = crate::testing::context();
        let offers = results_of(&["SOL", "SOLATI"]);
        let mut selection = holding(held);
        let mut at = None;
        for pass in 0..3 {
            let input = match (pass, at) {
                (2, Some(at)) => clicking(at),
                _ => egui::RawInput::default(),
            };
            let output = ctx.run_ui(input, |ui| {
                let mut travelled = None;
                let mut described = None;
                found(
                    ui,
                    &offers,
                    None,
                    &crate::map::index::Names::default(),
                    picking,
                    &mut selection,
                    &mut travelled,
                    &mut described,
                );
            });
            if at.is_none() {
                for (said, rect) in text_at(&output.shapes) {
                    if said == name {
                        at = Some(rect.center());
                    }
                }
            }
        }

        selection
    }

    /// Every word the pass painted, and where
    fn text_at(
        shapes: &[egui::epaint::ClippedShape],
    ) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    text.galley.text().to_owned(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for shape in shapes {
            walk(&shape.shape, &mut found);
        }
        found
    }

    /// A stop clicked in the route form is added to what is already picked
    ///
    /// Reported as a gap: the route runs between what is picked out, and
    /// while that mode was open there was no way to name a system. The field
    /// is there now, and what a click on its answer means is the whole of the
    /// difference between the two modes. A route wants two systems or more,
    /// so a click that let go of the last stop in order to take the next
    /// could never build one.
    #[test]
    fn a_stop_clicked_in_the_route_joins_what_is_picked() {
        let picked = line_clicked(Picking::Gathers, &["SOL"], "SOLATI");

        assert_eq!(
            stops_of(&picked),
            Ok(vec!["SOL", "SOLATI"]),
            "the stop did not join what was already picked out"
        );
    }

    /// Where a search picks out the one system the click named
    ///
    /// The other half of the same difference. A search is one name asked
    /// about and the system meant picked out, as a star on the map is, and
    /// the modifier is what gathers there.
    #[test]
    fn a_search_result_clicked_stands_in_for_what_was_picked() {
        let picked = line_clicked(Picking::Asked, &["SOL"], "SOLATI");

        assert_eq!(
            picked.systems().map(|system| system.name()).collect::<Vec<_>>(),
            vec!["SOLATI"],
            "the click gathered rather than picking one out"
        );
    }

    /// A box with something in it offers to empty itself
    #[test]
    fn a_box_holding_a_query_offers_to_clear_it() {
        assert!(box_said(Some("SOL"), &[]).iter().any(|line| line == CLOSE));
    }

    /// So does one whose query is gone but whose answer is still standing
    ///
    /// The list outlives what was typed, since picking a system out of it
    /// leaves it up to be picked from again. A mark that went with the query
    /// would leave the list with no way to dismiss it but typing.
    #[test]
    fn a_box_answered_by_a_list_offers_to_clear_it() {
        assert!(box_said(None, &["SOL"]).iter().any(|line| line == CLOSE));
    }

    /// An empty box offers nothing, having nothing to take away
    #[test]
    fn an_empty_box_offers_no_mark() {
        assert!(!box_said(None, &[]).iter().any(|line| line == CLOSE));
    }

    /// Clearing takes the query, the note and the list together
    ///
    /// All three answer the one name, so leaving any of them standing leaves
    /// an answer to a question no longer on screen.
    #[test]
    fn clearing_takes_everything_that_answered_the_name() {
        let mut value = Some("SOL".to_owned());
        let mut note = SearchNote(Some("No system named SOL".to_owned()));
        let mut results = results_of(&["SOLATI"]);

        cleared(&mut value, &mut note, &mut results);

        assert!(value.is_none());
        assert!(note.0.is_none());
        assert!(results.is_empty());
    }

    /// Nothing found draws nothing at all
    ///
    /// Rather than an empty box under the input. A search that found nothing
    /// is answered by the note, and a list standing empty beside it would be
    /// a second answer saying less.
    #[test]
    fn an_empty_list_is_not_drawn() {
        assert!(listed(&SearchResults::default(), None).is_empty());
    }

    /// The list paints in colors something can draw
    ///
    /// The distance at the end of a line is laid out apart from the line and
    /// painted against a placeholder, which is the arrangement that reaches
    /// the tessellator with nothing to draw and panics there.
    #[test]
    fn the_results_paint_in_colors() {
        painted(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &results(&["SOL", "NOWHERE"], false),
                Some(DVec3::ZERO),
                &crate::map::index::Names::default(),
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        });
    }

    /// And each line answers for itself
    #[test]
    fn the_result_lines_do_not_share_ids() {
        let said = crate::testing::complaints(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &results(&["SOL", "SOLATI", "SOLLARO"], true),
                None,
                &crate::map::index::Names::default(),
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        });

        assert!(said.is_empty(), "{said:?}");
    }
}
