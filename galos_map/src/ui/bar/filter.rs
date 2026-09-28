//! The filter form: a faction looked up by name, and the control over how
//! lately a system was updated

use crate::map::filter::{
    DimTo, FactionResults, Filter, Filters, Lookup, LookupNote, Resolving,
    SPANS, Standstill, Watch,
};
use crate::map::galaxy::spawn::PendingSpawns;
use crate::map::galaxy::{InReach, PendingEvictions};
use crate::map::route::SelectedFilter;
use crate::ui::FIELD_GAP;
use crate::ui::bar::search::OFFERED;
use crate::ui::list::{LINE_PADDING, line, scrolling};
use crate::ui::widgets::fill_width;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Response, Ui};
use galos_index::records::Faction as DbFaction;

/// The name the time control's own `Ui` is spelled out under
///
/// Global, so that what is inside it is numbered from this name alone and not
/// from how many widgets stand above it in the bar.
const WATCH: &str = "watch-control";

/// How much of the watch slider's row the span beside it is given
///
/// Wider than [`VALUE_WIDTH`](crate::ui::widgets::VALUE_WIDTH), the reading being a name rather than a number.
/// Enough for "15 minutes", the longest of [`SPANS`], which comes to 69.25 in
/// the face the map letters its controls in.
const SPAN_WIDTH: f32 = 70.;

/// The whole of the bar's filter section
///
/// One parameter for the same reason [`Settings`](crate::ui::settings::Settings) is one: a system may take
/// only
/// sixteen. Grouped by what it is about rather than by where it is drawn,
/// since the bar's three sections have little to say to each other and this
/// way none of them can reach into another's state by accident.
///
/// The count comes from [`InReach`] rather than being taken over the systems
/// here, since what the bar has to say is how much of the sky in front of the
/// user is getting through, and only [`crate::map::galaxy::visibility`] knows
/// which systems those are.
#[derive(SystemParam)]
pub(crate) struct FilterBar<'w, 's> {
    /// The filters themselves, which the rows are drawn from and changed in
    ///
    /// Named for what it holds rather than for its type, since this is
    /// reached through a parameter that is already about filters and
    /// `filter.filters` says the word twice and the thing once.
    pub(in crate::ui) active: ResMut<'w, Filters>,
    /// How much of the sky is getting through them
    pub(super) in_reach: Res<'w, InReach>,
    /// Whether systems are still being turned into stars, for the count's
    /// spinner
    pub(super) spawning: Res<'w, PendingSpawns>,
    /// Whether systems are being dropped off the map, for the count's arrow
    pub(super) evicting: Res<'w, PendingEvictions>,
    /// What is typed into the field that asks for one
    ///
    /// Here rather than among the bar's other fields, so that nothing about a
    /// filter is reachable through the search's state or the route's.
    pub(super) input: Local<'s, Option<String>>,
    /// Where a filter the user has typed is sent to be looked up
    pub(super) lookup: MessageWriter<'w, Lookup>,
    /// What became of the last one asked for
    pub(super) note: ResMut<'w, LookupNote>,
    /// The factions the last name typed might have meant
    pub(super) found: ResMut<'w, FactionResults>,
    /// Whether the name typed into it is still being looked up
    pub(super) pending: Res<'w, Resolving>,
    /// How faintly what they exclude is drawn
    pub(in crate::ui) dim: ResMut<'w, DimTo>,
    /// Where the control over time stands
    watch: ResMut<'w, Watch>,
    pub(super) standstill: ResMut<'w, Standstill>,
    /// Which filter the user is working with, which a click on a row says
    ///
    /// Only a route does anything with it today; the rest are picked out and
    /// nothing yet reads that they were.
    pub(super) chosen: ResMut<'w, SelectedFilter>,
    /// What a filter's systems are, for framing them
    ///
    /// The two tables `Filter::systems` answers from. Read here rather than
    /// worked out in the bar, a row asking to see a filter whole being a
    /// question about where its systems are and not about the row.
    pub(super) populated: Res<'w, crate::map::index::Populated>,
    pub(super) names: Res<'w, crate::map::index::Names>,
}

/// What the box asks in [`AskMode::Filter`](crate::ui::bar::AskMode::Filter), under the field
///
/// The field itself is the bar's one box, which is asking for a faction while
/// this mode is out: see [`ask_bar`](crate::ui::bar::ask_bar). What is left is what a name cannot say
/// — which of the factions holding it was meant, and how lately a system
/// must have been heard from — so this is the answer to the name and the one
/// filter that is not a name at all.
///
/// The field empties once a faction has been asked for. What was typed is a
/// row by then, and the field's next job is the next filter.
///
/// What went wrong is said here rather than beside the box, so that a name
/// that resolved to nothing is read under the name that did it, and the
/// search's own note cannot be mistaken for it: one box asks all three
/// questions, and only one of them is being asked at a time.
pub(super) fn filter_body(ui: &mut Ui, filter: &mut FilterBar) {
    if let LookupNote::Failed(why) = &*filter.note {
        ui.add_space(FIELD_GAP);
        ui.colored_label(egui::Color32::LIGHT_RED, why);
    }

    // A click chooses, as it does in every other list the map draws. The
    // search has already asked what the lookup would have asked, so the line
    // carries the id a filter tests against and there is nothing left to look
    // up: the faction goes straight into a row of its own.
    //
    // The field and the list go with it. What was typed is a row by now, and
    // the field's next job is the next faction.
    if let Some(faction) = faction_list(ui, filter.found.iter()) {
        filter.active.bypass_change_detection().add(Filter::Faction {
            id: faction.id,
            name: faction.name.clone(),
        });
        *filter.input = None;
        filter.found.clear();
    }

    watch_control(
        ui,
        &mut filter.watch,
        filter.active.bypass_change_detection(),
        &mut filter.standstill,
    );
}

/// Ask for a filter by how lately a system was updated
///
/// A slider over named spans rather than a field to type a time into. What is
/// being asked is roughly how fresh, and the spans are the answers anybody
/// wants: the far end of a typed time is a database going back years and the
/// near end is the last minute.
///
/// The label names what is being asked about and the span says how far back,
/// so the two read as "Last Updated" over "6 hours".
///
/// Only on a change, as the opacity beside it is. Asking is what marks the
/// filters as changed, and what reads that mark puts a fresh question to the
/// database.
///
/// Answers the slider, so that a caller can say whether it is being dragged.
pub(super) fn watch_control(
    ui: &mut Ui,
    watch: &mut Watch,
    active: &mut Filters,
    standstill: &mut Standstill,
) -> Response {
    ui.add_space(FIELD_GAP);
    ui.label("Last Updated");

    let mut standing = Watch(watch.0);
    fill_width(ui, SPAN_WIDTH);
    // Spelled out, and global so that the name is the whole of it. Egui
    // otherwise numbers a widget by how many were drawn before it, and this
    // one is drawn under the rows the filters make: asking for a filter here
    // adds a row above, every widget below it is renumbered, and egui follows
    // a drag by id. The slider the user pressed stops existing mid-gesture and
    // the drag is dropped.
    //
    // `push_id` does not answer it. A child `Ui` given a name still takes the
    // count it was made at into the ids of what it holds, so the widgets
    // inside are renumbered along with everything else.
    let slider = ui
        .scope_builder(
            egui::UiBuilder::new().id_salt(WATCH).global_scope(true),
            |ui| {
                ui.horizontal(|ui| {
                    let slider = ui.add(
                        egui::Slider::new(&mut standing.0, 0..=SPANS.len() - 1)
                            .show_value(false),
                    );
                    // Beside the slider rather than in the box egui draws its
                    // own reading in. That box is a field to type the value
                    // into, and what it would take is one of nine places along
                    // the slider, where what the user is reading is a span. So
                    // the box is turned off and the span said here.
                    //
                    // Read off the slider rather than off `watch`, which is
                    // not written until the drag is over: taken from there the
                    // name would say the span the slider set out from all the
                    // way through a drag.
                    ui.label(standing.name());
                    slider
                })
                .inner
            },
        )
        .inner;

    // Taken before the drag is allowed to ask for anything, so what is held
    // is how the rows stood when the press landed and not how they stood after
    // the first step of the drag had already added one.
    if slider.drag_started() {
        standstill.hold(active);
    }

    if slider.changed() {
        watch.0 = standing.0;
        match watch.span() {
            // Worked out here and not where it is asked. `Utc::now` is the one
            // thing in this that cannot be tested, so it is read at the one
            // place that turns a span into a moment.
            Some(span) => active.ask_within(watch.name(), span),
            // Its row stands above this control, so letting go of it mid-drag
            // would take the control up a row and out from under the pointer.
            // It stops asking where it stands instead, and says so.
            None if slider.dragged() => active.turn_time_off(watch.name()),
            None => active.ask_nothing_of_time(),
        }
    }

    // Let go of at the far end, so what stopped asking during the drag is let
    // go of now that moving the rows is nothing to the gesture.
    if slider.drag_stopped() {
        standstill.release();
        if watch.span().is_none() {
            active.ask_nothing_of_time();
        }
    }

    slider
}

/// The factions a search found, and which of them was clicked
///
/// Names alone. A faction is a name and an id, the id is what a filter tests
/// against rather than anything to read, and there is nothing else on record
/// about one worth a column.
///
/// Not [`system_list`](crate::ui::bar::search::system_list), which draws systems: a faction has nowhere to be, so
/// there is no distance to say, nothing to fly to and no panel of its own to
/// open. What the two share is the line they are drawn with.
pub(super) fn faction_list<'a>(
    ui: &mut Ui,
    factions: impl Iterator<Item = &'a DbFaction>,
) -> Option<&'a DbFaction> {
    // Nothing found is nothing drawn, as a list of systems is.
    let mut factions = factions.peekable();
    factions.peek()?;

    let height = ui.text_style_height(&egui::TextStyle::Body)
        + LINE_PADDING * 2.
        + ui.spacing().item_spacing.y;
    let mut chose = None;

    scrolling(ui, height * OFFERED as f32, "factions", |ui| {
        for faction in factions {
            // Keyed by where it sits, which is what `line` allocates itself
            // and what the lines of every other list are keyed by: a fresh
            // search leaves them where they were and makes each about
            // something else.
            let (_, answer) =
                line(ui, egui::RichText::new(faction.name.as_str()), 0., true);
            if answer.clicked() {
                chose = Some(faction);
            }
            answer.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    });

    chose
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{painted, words};

    /// The row over the set says how many are held, and is drawn over two
    ///
    /// The time control says which span it is standing on
    ///
    /// Egui draws a slider's own reading inside the box `show_value` turns
    /// off, and that box is turned off here because what it offers is a field
    /// to type a number into where these are named spans. Without a word
    /// beside it the control is nine positions and nothing to tell them apart.
    #[test]
    fn the_time_control_says_which_span_it_stands_on() {
        let mut watch = Watch(1);
        let mut active = Filters::default();
        let mut standstill = Standstill::default();

        let said = words(|ui| {
            watch_control(ui, &mut watch, &mut active, &mut standstill);
        });

        assert!(said.contains(&"30 days".to_owned()), "{said:?}");
    }

    /// And says so for the whole of its travel, the widest name included
    ///
    /// The slider is sized to leave [`SPAN_WIDTH`] for the name, which is the
    /// one thing that would quietly go wrong: a name too wide for the room
    /// left it is drawn off the end of the row it stands in.
    #[test]
    fn every_span_is_named_beside_the_control() {
        for (place, (name, _)) in SPANS.iter().enumerate() {
            let mut watch = Watch(place);
            let mut active = Filters::default();
            let mut standstill = Standstill::default();

            let said = words(|ui| {
                watch_control(ui, &mut watch, &mut active, &mut standstill);
            });

            assert!(said.contains(&(*name).to_owned()), "{name}: {said:?}");
        }
    }

    /// Left alone it asks nothing of time
    ///
    /// The moment a span works out to is read from the clock when the control
    /// is moved. Written every frame instead, it would move every frame and
    /// put a fresh question to the database each time.
    #[test]
    fn a_time_control_left_alone_asks_nothing() {
        let mut watch = Watch(1);
        let mut active = Filters::default();
        let mut standstill = Standstill::default();

        painted(|ui| {
            watch_control(ui, &mut watch, &mut active, &mut standstill);
        });

        assert_eq!(active.span(), None, "an untouched control asked");
        assert_eq!(active.iter().count(), 0, "a row appeared unasked");
    }
}
