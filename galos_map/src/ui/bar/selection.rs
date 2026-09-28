//! What is picked out, a row to each, under the bar

use crate::map::bodies::Contents;
use crate::map::camera::MoveCamera;
use crate::map::filter::{Filter, Filters};
use crate::map::selection::{Picked, SELECTION, Selection};
use crate::ui::DOT;
use crate::ui::bar::route::stops_of;
use crate::ui::bar::rows::{
    Buttons, buttons_width, lay_out_buttons, place_buttons, row_of,
};
use crate::ui::bar::{ROW_MARGIN, ROW_PADDING};
use crate::ui::list::{RowGesture, asked_of_row, scrolling};
use crate::ui::panels::Panels;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::Ui;

/// The color that dot is drawn in
///
/// [`SELECTION`] in egui's terms, so that the status line under the search
/// box and the ring out on the map are one mark in two places rather than
/// two colors to be matched up.
const SELECTION_DOT: egui::Color32 = egui::Color32::from_rgb(
    (SELECTION.red * 255.) as u8,
    (SELECTION.green * 255.) as u8,
    (SELECTION.blue * 255.) as u8,
);

/// Say what is picked out, and how far off it is
///
/// The status of the selection, which is not what the search box holds. The
/// box is a query, and a query answers with however many systems match it,
/// so it can never stand for the one system picked out. This says which that
/// is, in its own words, and goes on being right when a star is clicked on
/// the map and the box still holds whatever was last typed into it.
///
/// It is also what says a search worked. A search that resolves picks its
/// system out, and that shows up here.
///
/// The line is also the control that sends the camera to what is picked out:
/// double clicking the answer to go to what it names beats a button saying so
/// in words, and the dot in the ring's own color says which mark out on the
/// map it is about. The same press flies to a star on the map and frames a
/// filter's systems, so a row reads the way the thing it stands for does.
///
/// Measured from where the camera is looking rather than from the camera
/// itself, since that is the distance the spyglass and the fetch are
/// measured in: a system nearer than the spyglass radius is one that is
/// drawn.
/// Several picked out are several rows, each about one of them, so that no
/// row has to answer which system it means. Five of them and then scrolling,
/// as the results list is, and for the same reason: the bar hangs over the
/// map and a list long enough to reach the bottom of the viewport answers a
/// question by covering up what it is about.
///
/// The summary line above them is drawn only while more than one is picked
/// out, and carries [`whole_selection`]'s controls. One system picked out is
/// the case
/// the rows already read well, and a line saying "1 system" over a row naming
/// it says the same thing twice.
/// `travelled` is where a row asked the camera to go, which the caller writes
/// rather than this, as [`found`](crate::ui::bar::search::found) does and for the same reason. A whole move
/// rather than a place: a row says where alone, and the summary line's control
/// to frame the set says how much to take in as well.
///
/// Answers whether a route between what is picked out was asked for, which is
/// [`whole_selection`]'s to say and the caller's to act on: the form that
/// answers it
/// is drawn further down the bar.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn selected(
    ui: &mut Ui,
    selection: &mut Selection,
    contents: &Contents,
    center: Option<DVec3>,
    travelled: &mut Option<MoveCamera>,
    panels: &mut Panels,
    filters: &mut Filters,
    place: &mut usize,
) -> bool {
    if selection.is_empty() {
        return false;
    }

    let gap = ui.spacing().item_spacing.x;
    // Settled after the rows, since each is drawn from the same selection it
    // asks to change.
    let mut chose = None;
    // Where the column had reached, which is where what these rows spend of
    // it is counted from. See the end of this function.
    let from = *place;

    let routing = selection.len() > 1
        && whole_selection(ui, selection, filters, travelled);

    let height = ui.text_style_height(&egui::TextStyle::Body).max(DOT)
        + (ROW_PADDING + ROW_MARGIN) * 2.
        + ui.spacing().item_spacing.y;
    let mut rows = |ui: &mut Ui| {
        for index in 0..selection.len() {
            let Some(held) = selection.get(index) else { continue };

            // How far off it is, measured from the focus for both kinds and
            // said in whatever unit suits the range. A body inside a system
            // stands light seconds away where a system stands light years, and
            // either given in the other's unit is a number with too many
            // digits to read at a glance.
            //
            // Nothing else about a body stands on its row. What kind of thing
            // it is, and everything else on record, is the panel's to say.
            let beside =
                selection.position(index).zip(center).map(|(at, focus)| {
                    let away = focus.distance(at);
                    match held {
                        Picked::System(_) => format!("{away:.1} Ly"),
                        Picked::Body(_) => format!(
                            "{:.1} Ls",
                            crate::map::space::light_seconds(away)
                        ),
                    }
                });

            // Laid out and painted rather than assembled from labels. A label
            // is a widget in its own right, and two of them under one
            // clickable row leave three widgets bidding for the pointer: the
            // row answers over the gaps and the labels answer over the words,
            // so it flickers between being a control and not as the pointer
            // crosses them.
            let away = beside.map(|line| {
                egui::WidgetText::from(egui::RichText::new(line).weak())
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Body,
                    )
            });

            let buttons = lay_out_buttons(ui);
            let name = held.name();

            // Whatever the dot, the distance and the marks leave the name.
            // System names run to "Col 285 Sector XY-Z b12-34", and one laid
            // out against no bound at all is painted straight out past the
            // edge of the bar.
            let room = ui.available_width()
                - ROW_PADDING * 2.
                - DOT
                - gap
                - buttons_width(&buttons, gap)
                - away.as_ref().map_or(0., |away| away.size().x + gap);
            let name =
                egui::WidgetText::from(egui::RichText::new(name).strong())
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Truncate),
                        room.max(0.),
                        egui::TextStyle::Body,
                    );
            // Keyed on where the row sits in the bar rather than on what it
            // holds, and counted on from whatever came before it rather than
            // from this list's own first row. See [`row_of`].
            let of = ("bar-row", *place);
            *place += 1;
            let (outer, row) = row_of(
                ui,
                // The width the rest of the form is laid out in, so that the
                // row lines up with the fields above and below it rather than
                // being measured against anything of its own.
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
            ui.painter().circle_filled(
                egui::pos2(x + DOT / 2., middle),
                DOT / 2.,
                SELECTION_DOT,
            );
            x += DOT + gap;
            for galley in [Some(name), away].into_iter().flatten() {
                let size = galley.size();
                // The galleys carry the colors they were laid out in, so
                // there is nothing for a fallback to answer for.
                ui.painter().galley(
                    egui::pos2(x, middle - size.y / 2.),
                    galley,
                    egui::Color32::PLACEHOLDER,
                );
                x += size.x + gap;
            }

            let Buttons { info, close, .. } =
                place_buttons(ui, rect, buttons, of);

            let asked = asked_of_selection(
                close.clicked(),
                info.is_some_and(|info| info.clicked()),
                row.double_clicked(),
                row.clicked(),
            );
            if let Some(asked) = asked {
                chose = Some((index, asked));
            }
            row.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    };

    // Only once there are more than the bar holds. A scroll area around three
    // rows is a scroll area that never scrolls and takes a little room off
    // the end of every one of them for a bar that is not there.
    if selection.len() > SELECTED {
        scrolling(ui, height * SELECTED as f32, "selection", &mut rows);
    } else {
        rows(ui);
    }

    // What the column spent on them, which is what the rows below it are
    // numbered from, and which is not how many rows were drawn: past
    // [`SELECTED`] they are drawn inside a scroll area of a fixed height, so
    // the seventh system picked out moves nothing below it.
    //
    // The count has to move with the rows below rather than with the rows
    // here, that being the whole of what it is for. Gathering another system
    // while the list scrolls would otherwise put a fresh id at a filter row
    // that kept its rectangle, which is what egui reads as one widget taking
    // another's state: it says so out loud and paints the row red.
    //
    // The rows drawn inside the scroll area are numbered on past this and
    // come to no harm by it. They are keyed within the scroll area's own
    // `Ui`, so what they are numbered has nothing to say about anything
    // drawn outside it.
    *place = from + selection.len().min(SELECTED);

    if let Some((index, action)) = chose {
        match action {
            SelectionAction::Travel => {
                *travelled = selection.position(index).map(|position| {
                    MoveCamera { position: Some(position), framing: None }
                })
            }
            // Whatever the row is about. A system is described from the row
            // the bar holds; what is inside one is described from the rows the
            // map is holding, which it has for as long as the thing is drawn,
            // and a row for one is only held for that long either.
            SelectionAction::Describe => match selection.get(index) {
                Some(Picked::System(system)) => {
                    panels.open_system(system.clone())
                }
                Some(Picked::Body(body)) => {
                    if let Some(star) = contents.star(body.id()) {
                        panels.open_star(star.clone());
                    } else if let Some(row) = contents.body(body.id()) {
                        panels.open_body(row.clone());
                    }
                }
                None => {}
            },
            SelectionAction::LetGo => selection.remove(index),
        }
    }

    routing
}

/// What the bar can be asked to do with one selected system
///
/// Said by index, several rows standing at once and each being about one of
/// them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SelectionAction {
    /// Send the camera to it, as double clicking its star does
    Travel,
    /// Open the panel describing it
    Describe,
    /// Let go of this one, and hold the rest
    LetGo,
}

/// What a press on one selected system's row asked of it
///
/// The same reading [`asked_of_row`] gives a filter's row, which is where the
/// order is written down: the mark, then the button, then the double, then
/// the click. A row here has no switch inside it -- what it stands for is
/// picked out by standing there at all -- so it is the one place a row can be
/// pressed that this leaves out.
///
/// The camera is what a double asks for, as a double on the star itself asks,
/// rather than what a single click asks as it used to. A row about one system
/// has the whole of that system in view already, so the gesture that frames a
/// filter is a flight to this one.
///
/// A single click asks for nothing. On a filter's row it says which of several
/// is the one being worked with; here there is nothing for it to say, the row
/// standing for something already picked out. Left as a gesture with no
/// answer rather than given the camera back, so that the same press means the
/// same thing wherever in the bar it lands.
fn asked_of_selection(
    close: bool,
    info: bool,
    double: bool,
    click: bool,
) -> Option<SelectionAction> {
    match asked_of_row(close, info, false, false, double, click) {
        Some(RowGesture::LetGo) => Some(SelectionAction::LetGo),
        Some(RowGesture::Describe) => Some(SelectionAction::Describe),
        Some(RowGesture::Frame) => Some(SelectionAction::Travel),
        // A system picked out was never searched for and is not turned off:
        // its row offers neither, and says so by never reporting them.
        Some(RowGesture::Replot | RowGesture::Toggle | RowGesture::Select)
        | None => None,
    }
}

/// How many selected systems the bar shows before the rows start scrolling
const SELECTED: usize = 5;

/// One row standing for everything picked out, and what it offers
///
/// Says how many there are and offers to bring the map to bear on them.
/// `whole_set` is the same shape over the filter rows.
///
/// The set is left alone once it has been filtered on. The filter took a copy
/// of the addresses, so letting go of the rings and the rows afterwards
/// leaves those systems picked out, which is most of what the filter is for.
///
/// Answers whether a route through them was asked for. A route wants two of
/// them at the least, so the control appears the moment the second is picked
/// and stays as more are gathered. That is what there is to find: a set
/// gathered out on the map says here what can be done with it, rather than
/// leaving the user to guess that the form dropping out of the search box has
/// a section about the systems they have already picked.
///
/// It reaches the form rather than plotting, since a route still wants a jump
/// range and there is nowhere here to say one.
///
/// And offers to frame the lot: to stand the camera back over the middle of
/// what is picked out, far enough to take all of it in. A space walks the set
/// one at a time and says nothing about how far out to stand, and a set
/// gathered across the galaxy is one the user wants to see whole before
/// walking it. Systems and bodies alike, a body being somewhere as much as a
/// system is. Not offered where they all stand in one place, there being
/// nothing to stand back from and a frame over nothing being a camera pulled
/// in to a metre.
fn whole_selection(
    ui: &mut Ui,
    selection: &Selection,
    filters: &mut Filters,
    travelled: &mut Option<MoveCamera>,
) -> bool {
    // The systems alone, [`Filter`] naming systems by address and testing a
    // [`System`]. A body is counted among what is picked out, and there is as
    // yet no filter for it to build.
    let picked = selection.systems().count();
    let mut routing = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{picked} systems")).weak());
        // Offered only where there is a system to filter on. A set of bodies
        // builds a filter over no addresses, which admits nothing, and the map
        // fetches by the same answer it dims by: the sky goes black under a
        // row saying none was picked. Widen this when a filter can name a
        // body, rather than dropping it.
        if picked > 0 && ui.button("Filter").clicked() {
            filters.add(Filter::Systems {
                label: format!("{picked} systems"),
                systems: selection.addresses(),
            });
        }
        if stops_of(selection).is_ok() {
            routing = ui.button("Route").clicked();
        }
        if let Some((middle, extent)) = spanned(selection)
            && ui.button("Frame").clicked()
        {
            *travelled = Some(MoveCamera {
                position: Some(middle),
                framing: Some(extent),
            });
        }
    });
    routing
}

/// The middle of everything picked out, and how far it reaches from there
///
/// Nothing where it reaches nowhere: one thing alone, or several standing in
/// the one place, which is a place to fly to rather than a span to take in.
///
/// The same [`crate::map::route::spawn::framing`] a plotted route is
/// framed by, so a set and the route through it are stood back from the same
/// way.
fn spanned(selection: &Selection) -> Option<(DVec3, f32)> {
    let places: Vec<DVec3> = (0..selection.len())
        .filter_map(|index| selection.position(index))
        .collect();
    let (middle, extent) = crate::map::route::spawn::framing(&places)?;

    (extent > 0.).then_some((middle, extent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::words;
    use crate::ui::testing::{body, draw_selected, holding, strung_out};

    /// What the bar says about a selection holding `names`
    fn selection_said(names: &[&str]) -> Vec<String> {
        let mut selection = holding(names);

        words(|ui| {
            let mut panels = Panels::default();
            let mut filters = Filters::default();
            let mut travelled = None;
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut travelled,
                &mut panels,
                &mut filters,
                &mut 0,
            );
        })
    }

    /// Every system picked out gets a row naming it
    ///
    /// A row apiece rather than one row about several, so that no row has to
    /// answer which of them it means.
    #[test]
    fn every_selected_system_gets_a_row() {
        let said = selection_said(&["SOL", "ALPHA CENTAURI", "BARNARD"]);

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(said.contains(&"ALPHA CENTAURI".to_owned()), "{said:?}");
        assert!(said.contains(&"BARNARD".to_owned()), "{said:?}");
    }

    /// What the bar says about `picked` being picked out
    fn rows_said(picked: &[Picked]) -> Vec<String> {
        let mut selection = Selection::default();
        for one in picked {
            selection.toggle(one.clone());
        }

        words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        })
    }

    /// Every body picked out gets a row naming it
    ///
    /// The same row a system gets and in the same list, since a body is picked
    /// out by the same gesture. The rings are out on the map where the user is
    /// looking, and the bar is where what is picked out is read.
    #[test]
    fn every_picked_body_gets_a_row() {
        let said = rows_said(&[body(3, "SOL 3", 0.), body(4, "SOL 4", 0.)]);

        assert!(said.contains(&"SOL 3".to_owned()), "{said:?}");
        assert!(said.contains(&"SOL 4".to_owned()), "{said:?}");
    }

    /// A body's row says how far off it is in light seconds
    ///
    /// Where a system's row says light years, and measured from the same
    /// focus. A light second is about a thirty millionth of a light year, so
    /// a body given in light years is a row of leading zeroes.
    #[test]
    fn a_body_row_says_how_far_off_it_is_in_light_seconds() {
        // A hundred light seconds, in the light years the map measures in.
        let away = 100. / crate::map::space::light_seconds(1.);
        let mut selection = Selection::default();
        selection.toggle(body(3, "SOL 3", away));

        let said = words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert!(said.contains(&"100.0 Ls".to_owned()), "{said:?}");
    }

    /// A system and a body picked out together are one list of rows
    ///
    /// Which is the whole of what holding them the same way buys: the bar
    /// draws what is picked out, in the order it was picked, without asking
    /// what kind each of them is except to say what stands beside the name.
    #[test]
    fn a_system_and_a_body_share_the_one_list() {
        let mut selection = holding(&["SOL"]);
        selection.toggle(body(3, "SOL 3", 0.));

        let said = words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert_eq!(selection.len(), 2);
        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(said.contains(&"SOL 3".to_owned()), "{said:?}");
    }

    /// The rows each answer for themselves, whatever they hold
    #[test]
    fn the_body_rows_do_not_share_ids() {
        let mut selection = holding(&["SOL"]);
        selection.toggle(body(3, "SOL 3", 0.));
        selection.toggle(body(4, "SOL 4", 0.));

        let said = crate::testing::complaints(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert!(said.is_empty(), "{said:?}");
    }

    /// A row says how far off its system is, in as few words as that takes
    ///
    /// The number and the unit and nothing else. The rows of every other list
    /// the map draws end the same way, and what stands at the end of a row is
    /// read as the distance whether or not a word says so, so a word saying so
    /// is a word taking room from the name beside it.
    #[test]
    fn a_selection_row_says_how_far_off_its_system_is() {
        let mut selection = Selection::default();
        selection.toggle(Picked::System(crate::map::galaxy::tests::at(1, 12.)));

        let said = words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert!(said.contains(&"12.0 Ly".to_owned()), "{said:?}");
    }

    /// Several picked out says how many, and offers to filter on them
    #[test]
    fn a_gathered_selection_offers_to_filter_on_itself() {
        let said = selection_said(&["SOL", "BARNARD"]);

        assert!(said.contains(&"2 systems".to_owned()), "{said:?}");
        assert!(said.contains(&"Filter".to_owned()), "{said:?}");
    }

    /// One picked out says neither
    ///
    /// The row already names it, and a line saying "1 system" over a row
    /// naming that system says the same thing twice. There is nothing to
    /// gather either, a filter over one system being the system itself.
    #[test]
    fn one_selected_system_is_left_to_its_own_row() {
        let said = selection_said(&["SOL"]);

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(!said.contains(&"1 systems".to_owned()), "{said:?}");
        assert!(!said.contains(&"Filter".to_owned()), "{said:?}");
    }

    /// Two picked out are offered a route between them
    ///
    /// This is where the feature is found. A user who gathers two systems out
    /// on the map has said everything a route needs but the jump range, and
    /// nothing else on screen would tell them the form dropping out of the
    /// search box has a section about the pair they are already holding.
    #[test]
    fn two_selected_systems_are_offered_a_route() {
        let said = selection_said(&["SOL", "BARNARD"]);

        assert!(said.contains(&"Route".to_owned()), "{said:?}");
    }

    /// Bodies alone are offered no filter, while none can name them
    ///
    /// A body is picked out into the same list as a system and counted with
    /// it, so a pair of them reaches the gathered controls while leaving no
    /// system to gather. The filter that would build names no address, admits
    /// nothing, and blanks the sky under a row saying none was picked, the map
    /// fetching by the same answer it dims by.
    ///
    /// About what a filter can name rather than about bodies. A filter that
    /// can name one makes this case a filter over bodies, and this test the
    /// wrong question.
    #[test]
    fn bodies_alone_are_not_offered_a_filter() {
        let said = rows_said(&[body(1, "SOL A", 0.), body(2, "SOL B", 0.)]);

        assert!(said.contains(&"0 systems".to_owned()), "{said:?}");
        assert!(!said.contains(&"Filter".to_owned()), "{said:?}");
    }

    /// One alone is not, there being no route it could ask for
    ///
    /// A control that leads to a form refusing what it just asked for is
    /// worse than no control: it says the map can do something it cannot.
    /// More than two is a route through all of them, and is offered one.
    #[test]
    fn a_set_that_cannot_be_routed_is_offered_no_route() {
        let alone = selection_said(&["SOL"]);
        let several = selection_said(&["SOL", "BARNARD", "WOLF 359"]);

        assert!(!alone.contains(&"Route".to_owned()), "{alone:?}");
        assert!(several.contains(&"Route".to_owned()), "{several:?}");
        // The rest of the line stands either way.
        assert!(several.contains(&"Filter".to_owned()), "{several:?}");
    }

    /// A set that spans somewhere is offered a frame over the whole of it
    ///
    /// Systems and bodies alike, both being somewhere. Several standing in
    /// one place are not offered it: a frame over nothing is a camera pulled
    /// in to a metre. Nor is one alone, there being no summary line to offer
    /// it from.
    #[test]
    fn a_set_that_spans_somewhere_is_offered_a_frame() {
        let spread = rows_said(&[body(1, "SOL A", 0.), body(2, "SOL B", 5.)]);
        let heaped = rows_said(&[body(1, "SOL A", 3.), body(2, "SOL B", 3.)]);
        let alone = selection_said(&["SOL"]);

        assert!(spread.contains(&"Frame".to_owned()), "{spread:?}");
        assert!(!heaped.contains(&"Frame".to_owned()), "{heaped:?}");
        assert!(!alone.contains(&"Frame".to_owned()), "{alone:?}");
    }

    /// What a frame takes in is the middle of the set and its reach from there
    ///
    /// Which is what tells a frame from a flight: a row says where alone, and
    /// this says how much to stand back for as well.
    #[test]
    fn a_frame_stands_over_the_middle_of_the_set() {
        assert_eq!(
            spanned(&strung_out(&[0., 10., 4.])),
            Some((DVec3::new(5., 0., 0.), 5.))
        );
        assert_eq!(spanned(&strung_out(&[7.])), None);
        assert_eq!(spanned(&Selection::default()), None);
    }

    /// The selection rows each answer for themselves
    ///
    /// Both over a pair and over a longer set, since a pair carries a control
    /// the longer set does not and two controls in the one summary line are
    /// two more things to collide.
    #[test]
    fn the_selection_rows_do_not_share_ids() {
        for names in [&["SOL", "BARNARD"][..], &["SOL", "BARNARD", "WOLF 359"]]
        {
            let mut selection = holding(names);

            let said = crate::testing::complaints(|ui| {
                let mut panels = Panels::default();
                let mut filters = Filters::default();
                let mut travelled = None;
                selected(
                    ui,
                    &mut selection,
                    &Contents::default(),
                    None,
                    &mut travelled,
                    &mut panels,
                    &mut filters,
                    &mut 0,
                );
            });

            assert!(said.is_empty(), "{names:?}: {said:?}");
        }
    }

    /// Nothing picked out draws no rows at all
    #[test]
    fn an_empty_selection_draws_nothing() {
        assert!(selection_said(&[]).is_empty());
    }

    /// The rows keep their ids as the set outgrows what the bar shows at once
    ///
    /// Past [`SELECTED`] the rows are drawn inside a scroll area, and a row's
    /// id is taken from the `Ui` it is drawn in. The rows keep their
    /// rectangles across that change, so an id taken from the scroll area's
    /// own `Ui` would be a new id at an old rectangle.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn outgrowing_the_bar_does_not_change_the_row_ids() {
        let five = ["SOL", "BARNARD", "WOLF 359", "LUYTEN", "ROSS 128"];
        let six =
            ["SOL", "BARNARD", "WOLF 359", "LUYTEN", "ROSS 128", "LALANDE"];

        let said = crate::testing::between_passes(
            draw_selected(&five),
            draw_selected(&six),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// And as the summary above them comes and goes
    ///
    /// Gathering a second system stands a line saying how many over the rows,
    /// which moves every one of them down a line.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn gathering_a_second_system_does_not_change_the_row_ids() {
        let said = crate::testing::between_passes(
            draw_selected(&["SOL"]),
            draw_selected(&["SOL", "BARNARD"]),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// A selected system is flown to by a double click, not a single one
    ///
    /// The same press that frames a filter, said of the one system the row
    /// stands for. Every case here carries the click as well: the buttons sit
    /// inside the row and egui answers the first click of a pair as a click,
    /// so a press that means anything else arrives with one beside it and has
    /// to beat it.
    #[test]
    fn a_selected_system_is_flown_to_by_a_double_click() {
        assert_eq!(
            asked_of_selection(false, false, true, true),
            Some(SelectionAction::Travel)
        );
        assert_eq!(
            asked_of_selection(true, false, false, true),
            Some(SelectionAction::LetGo)
        );
        assert_eq!(
            asked_of_selection(false, true, false, true),
            Some(SelectionAction::Describe)
        );

        // A click on the name alone, which used to fly the camera there. The
        // row already stands for something picked out, so it asks nothing.
        assert_eq!(asked_of_selection(false, false, false, true), None);
        assert_eq!(asked_of_selection(false, false, false, false), None);
    }
}
