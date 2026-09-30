//! The filter form: the color key, a faction looked up by name, and the
//! control over how lately a system was updated

use crate::map::filter::key::{Hidden, Item, Tier, held_tiers};
use crate::map::filter::mask::{Held, Mask};
use crate::map::filter::{
    DimTo, FactionResults, Filter, Filters, Lookup, LookupNote, Resolving,
    SPANS, Standstill, Watch,
};
use crate::map::galaxy::spawn::{ColorBy, PendingSpawns};
use crate::map::galaxy::{InReach, PendingEvictions};
use crate::map::index::{ResidentIndex, Settled};
use crate::map::route::SelectedFilter;
use crate::ui::FIELD_GAP;
use crate::ui::bar::search::OFFERED;
use crate::ui::legend::{Swatch, attention, showing};
use crate::ui::list::{LINE_PADDING, line, scrolling};
use crate::ui::text::thousands;
use crate::ui::widgets::fill_width;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Response, Ui};
use galos_index::prelude::{CellId, StarKind};
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
    /// The color key, in a parameter of its own: this one is at the sixteen
    /// a system may take
    pub(in crate::ui) key: ColorKey<'w, 's>,
}

/// What the color key is drawn from, beside the mask [`Filters`] holds
#[derive(SystemParam)]
pub(crate) struct ColorKey<'w, 's> {
    /// Which axis the map is colored by, which the key's dropdown chooses
    ///
    /// Set only on a change, since the blobs are rebuilt on one.
    pub(in crate::ui) color_by: ResMut<'w, ColorBy>,
    /// The colonies counted, for the counts beside each value
    settled: Res<'w, Settled>,
    /// Every system counted, for the systems nobody lives in
    index: Res<'w, ResidentIndex>,
    pub(super) state: Local<'s, KeyState>,
}

impl ColorKey<'_, '_> {
    /// What the galaxy holds to count the key's values in, and how many
    /// systems nobody lives in
    ///
    /// The root histograms are resident, so asking every frame costs nothing.
    /// [`None`] while the colonies are still being read.
    pub(in crate::ui) fn counted(&self) -> (Option<Held>, u64) {
        let root = self.index.0.get(CellId::ROOT);
        let stellar = root.map_or(0, |cell| cell.aggregate.count());
        let stars =
            root.map_or([0; StarKind::COUNT], |cell| *cell.aggregate.kinds());
        let held = self
            .settled
            .0
            .get(CellId::ROOT)
            .map(|colonies| Held { colonies: *colonies, stars });
        let peopled = held.map_or(0, |held| held.colonies.count());
        (held, stellar.saturating_sub(peopled))
    }
}

/// The key's own state, which outlives a pass over it
#[derive(Default)]
pub(super) struct KeyState {
    /// Whether Other is unfolded, which it is not until asked
    other_open: bool,
    /// Whether the color row asked for the key, for the ask bar to open
    ///
    /// The rows are drawn after the bar, so the form comes out a frame
    /// later, the same lateness a route asked for from the rows has.
    pub(super) opening: bool,
    /// Whether the key was out on the last pass over the bar, which is
    /// when the color row keeps its mini legend to itself
    pub(super) out: bool,
}

/// What a click in the key or on the color row asked of the mask
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Keyed {
    /// Hide every one of these if any is shown, else show them all
    Toggle(Vec<usize>),
    /// Hide everything else in the axis, and the uninhabited too
    Solo(Vec<usize>),
    Uninhabited,
    ShowAll,
    HideAll,
    Invert,
}

impl Keyed {
    /// A click on `buckets`, a solo where a modifier was held
    pub(super) fn clicked(ui: &Ui, buckets: Vec<usize>) -> Keyed {
        if soloing(ui) { Keyed::Solo(buckets) } else { Keyed::Toggle(buckets) }
    }

    /// Carry it out on `filters`' mask, along `axis`
    ///
    /// Through [`Filters::edit_mask`], which is what says the filters moved,
    /// and only where they did.
    pub(super) fn apply(self, filters: &mut Filters, axis: ColorBy) {
        filters.edit_mask(|mask| match self {
            Keyed::Toggle(buckets) => mask.toggle(axis, &buckets),
            Keyed::Solo(buckets) => mask.solo(axis, &buckets),
            Keyed::Uninhabited => {
                mask.set_uninhabited(!mask.hides_uninhabited())
            }
            Keyed::ShowAll => mask.show_all(axis),
            Keyed::HideAll => mask.hide_all(axis),
            Keyed::Invert => mask.invert(axis),
        });
    }
}

/// Whether a click is asking for one value alone
///
/// Alt, ctrl or cmd. Not shift, which the map already reads as gathering,
/// and a solo is the opposite of adding to what is there.
fn soloing(ui: &Ui) -> bool {
    ui.input(|input| {
        let keys = input.modifiers;
        keys.alt || keys.ctrl || keys.command
    })
}

/// How tall the key's list grows before it scrolls
///
/// Government is fifteen rows under eight headers, and at full length it
/// pushed the time control under it off the bottom of a laptop's screen.
const KEY_HEIGHT: f32 = 360.;

/// How large a swatch stands in the key
const KEY_SWATCH: f32 = 10.;

/// How wide the room the Other chevron stands in
const CHEVRON: f32 = 14.;

/// What the box asks in [`AskMode::Filter`](crate::ui::bar::AskMode::Filter), under the field
///
/// The faction's answers first, directly under the box they answer: a name
/// being typed is read against what it found, and a list below a key the
/// height of the government axis is a list off the bottom of the screen.
/// Then the color key: which axis the map is colored by, and a toggle for
/// every color along it. Then the control over time.
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

    // Worked on through a copy, so that a pass which chose nothing does not
    // mark the axis changed and rebuild the blobs. Only while the map is
    // colored: the realistic view has no color for a key to name.
    if filter.active.mask().drawn().is_some() {
        let mut axis = *filter.key.color_by;
        let (held, empty) = filter.key.counted();
        let asked = key(
            ui,
            filter.active.mask(),
            &mut axis,
            held.as_ref(),
            empty,
            &mut filter.key.state.other_open,
        );
        filter.key.color_by.set_if_neq(axis);
        if let Some(asked) = asked {
            asked.apply(filter.active.bypass_change_detection(), axis);
        }
        ui.separator();
    }

    watch_control(
        ui,
        &mut filter.watch,
        filter.active.bypass_change_detection(),
        &mut filter.standstill,
    );
}

/// The color key: a dropdown of the axes, a row for each of the chosen one's
/// values, the systems nobody lives in, and what can be done to the axis at
/// once
///
/// Answers what a click asked of the mask, carried out by the caller since
/// the rows are drawn from the mask it changes.
///
/// `held` is what the galaxy holds, whatever has been counted of it, and
/// `empty` how many systems nobody lives in; `other_open` whether Other is
/// unfolded.
pub(super) fn key(
    ui: &mut Ui,
    mask: &Mask,
    axis: &mut ColorBy,
    held: Option<&Held>,
    empty: u64,
    other_open: &mut bool,
) -> Option<Keyed> {
    ui.add_space(FIELD_GAP);
    // One control whatever the bar's width: eight axes as tabs wrapped onto
    // a second line, leaving one of them alone under the rest. Unlabeled, the
    // axis it shows being label enough.
    egui::ComboBox::from_id_salt("color-by")
        .selected_text(axis.name())
        .show_ui(ui, |ui| {
            for offered in ColorBy::ALL {
                // Star class is not offered while only colonies are drawn;
                // see `crate::map::filter::follow_color_by`.
                let drawn = !offered.every_system() || mask.draws_uninhabited();
                ui.add_enabled_ui(drawn, |ui| {
                    ui.selectable_value(&mut *axis, offered, offered.name())
                })
                .inner
                .on_disabled_hover_text(
                    "Star class colors every system, and only colonies are \
                     drawn while scaling with population",
                );
            }
        });
    let axis = *axis;
    let mut asked = None;

    let tiers = held_tiers(axis, held);
    // Room for the chevron beside every top-tier row where any of them folds,
    // so the swatches stand in one column whether or not a row has one.
    let gutter = if tiers
        .iter()
        .any(|tier| matches!(tier, Tier::Group { collapsible: true, .. }))
    {
        CHEVRON
    } else {
        0.
    };
    // A member is drawn in under its header's name, past the header's swatch.
    let member = gutter + KEY_SWATCH + ui.spacing().item_spacing.x;
    let count = |counted: u64| held.map(|_| thousands(counted));

    scrolling(ui, KEY_HEIGHT, "color-key", |ui| {
        for (place, tier) in tiers.iter().enumerate() {
            match tier {
                Tier::Item(item) => {
                    let row = item_row(
                        ui,
                        ("key-item", place),
                        gutter,
                        item,
                        axis,
                        mask,
                        held,
                    );
                    if row.clicked() {
                        asked = Some(Keyed::clicked(ui, item.buckets.clone()));
                    }
                }
                Tier::Group { collapsible: true, items, .. } => {
                    let hidden = tier.hidden(axis, mask);
                    let (row, rect) = key_line(
                        ui,
                        ("key-group", place),
                        gutter,
                        &Swatch::of_tier(tier, axis, mask),
                        named(tier.name(), hidden),
                        showing(hidden).map(|said| {
                            egui::RichText::new(said)
                                .small()
                                .color(attention(ui))
                        }),
                        held.and_then(|held| count(tier.count(axis, held))),
                    );
                    // In front of the row, so the chevron takes its own press.
                    let chevron = ui.interact(
                        egui::Rect::from_min_size(
                            rect.min,
                            egui::vec2(CHEVRON + LINE_PADDING, rect.height()),
                        ),
                        ui.id().with(("key-chevron", place)),
                        egui::Sense::click(),
                    );
                    chevron_mark(
                        ui,
                        egui::pos2(
                            rect.left() + LINE_PADDING + CHEVRON / 2.,
                            rect.center().y,
                        ),
                        *other_open,
                    );
                    if chevron.clicked() {
                        *other_open = !*other_open;
                    } else if row.clicked() {
                        asked = Some(Keyed::clicked(ui, tier.buckets()));
                    }
                    chevron.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if *other_open {
                        for (at, item) in items.iter().enumerate() {
                            let row = item_row(
                                ui,
                                ("key-member", place, at),
                                member,
                                item,
                                axis,
                                mask,
                                held,
                            );
                            if row.clicked() {
                                asked = Some(Keyed::clicked(
                                    ui,
                                    item.buckets.clone(),
                                ));
                            }
                        }
                    }
                }
                // A government's color: its name over its members, always
                // open, the members being what the reader came to toggle.
                Tier::Group { items, .. } => {
                    let hidden = tier.hidden(axis, mask);
                    let (said, color) = match hidden {
                        Hidden::None => {
                            ("all".to_owned(), ui.visuals().weak_text_color())
                        }
                        Hidden::All => ("none".to_owned(), attention(ui)),
                        Hidden::Some { .. } => {
                            (showing(hidden).unwrap_or_default(), attention(ui))
                        }
                    };
                    let (row, _) = key_line(
                        ui,
                        ("key-group", place),
                        gutter,
                        &Swatch::of_tier(tier, axis, mask),
                        egui::RichText::new(tier.name().to_uppercase())
                            .small()
                            .weak(),
                        Some(egui::RichText::new(said).small().color(color)),
                        None,
                    );
                    if row.clicked() {
                        asked = Some(Keyed::clicked(ui, tier.buckets()));
                    }
                    for (at, item) in items.iter().enumerate() {
                        let row = item_row(
                            ui,
                            ("key-member", place, at),
                            member,
                            item,
                            axis,
                            mask,
                            held,
                        );
                        if row.clicked() {
                            asked =
                                Some(Keyed::clicked(ui, item.buckets.clone()));
                        }
                    }
                }
            }
        }
    });

    // Under a hairline, being no value of the axis: the same systems whichever
    // political axis is out. Star class has no such row, every system having
    // a star.
    ui.separator();
    if !axis.every_system() {
        asked = uninhabited_row(ui, mask, gutter, held, empty).or(asked);
    }

    ui.horizontal(|ui| {
        for (said, what) in [
            ("all", Keyed::ShowAll),
            ("none", Keyed::HideAll),
            ("invert", Keyed::Invert),
        ] {
            let pressed = ui
                .add(
                    egui::Label::new(egui::RichText::new(said).small())
                        .selectable(false)
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if pressed.clicked() {
                asked = Some(what);
            }
        }
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new("alt-click: solo").small().weak());
            },
        );
    });

    asked
}

/// The systems nobody lives in, under a political axis's values: a toggle
/// where they are drawn, and a word on why they are not where they are not
fn uninhabited_row(
    ui: &mut Ui,
    mask: &Mask,
    gutter: f32,
    held: Option<&Held>,
    empty: u64,
) -> Option<Keyed> {
    if !mask.draws_uninhabited() {
        // Said rather than left out without a word: a row that was there
        // and is gone reads as the key having lost it.
        ui.label(
            egui::RichText::new(
                "Uninhabited systems are not drawn while scaling with \
                 population",
            )
            .small()
            .weak(),
        );
        return None;
    }
    let (row, _) = key_line(
        ui,
        "key-uninhabited",
        gutter,
        &Swatch::uninhabited(mask),
        named(
            "Uninhabited",
            if mask.hides_uninhabited() { Hidden::All } else { Hidden::None },
        ),
        None,
        held.map(|_| thousands(empty)),
    );
    // Said on the row, beside the gray value of every axis it could be taken
    // for: those are colonies with nothing on record, and these are systems
    // nobody lives in at all.
    let row = row.on_hover_text(
        "Systems nobody lives in. Colonies with no allegiance, government or \
         security on record are the gray row above.",
    );
    row.clicked().then_some(Keyed::Uninhabited)
}

/// A value's name, struck through and muted where it is wholly hidden
fn named(name: &str, hidden: Hidden) -> egui::RichText {
    let text = egui::RichText::new(name);
    match hidden {
        Hidden::All => text.strikethrough().weak(),
        _ => text,
    }
}

/// One value's row: its swatch, its name, and how many colonies it counts
fn item_row(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    indent: f32,
    item: &Item,
    axis: ColorBy,
    mask: &Mask,
    held: Option<&Held>,
) -> Response {
    let hidden = item.hidden(axis, mask);
    key_line(
        ui,
        id,
        indent,
        &Swatch::of_item(item, axis, mask),
        named(item.name, hidden),
        None,
        held.map(|held| thousands(item.count(axis, held))),
    )
    .0
}

/// One full-width line of the key, and the pointer's answer to it
///
/// Laid out, then answered over the whole of it, so the line is one control
/// rather than a swatch and a label each bidding for the press: see
/// [`line`]. Answers the line's rectangle beside, for what the caller lays
/// in front of it.
///
/// `tail` is what stands at the right hand end, past the count.
fn key_line(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    indent: f32,
    swatch: &Swatch,
    name: egui::RichText,
    tail: Option<egui::RichText>,
    count: Option<String>,
) -> (Response, egui::Rect) {
    let height =
        ui.text_style_height(&egui::TextStyle::Body) + LINE_PADDING * 2.;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    if ui.rect_contains_pointer(rect) {
        ui.painter().rect_filled(
            rect,
            ui.visuals().widgets.hovered.corner_radius,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    let inner = rect.shrink2(egui::vec2(LINE_PADDING, 0.));
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inside.add_space(indent);
    swatch.paint(&mut inside, KEY_SWATCH);
    inside.add(egui::Label::new(name).selectable(false).truncate());
    inside.with_layout(
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            if let Some(tail) = tail {
                ui.add(egui::Label::new(tail).selectable(false));
            }
            if let Some(count) = count {
                ui.add(
                    egui::Label::new(egui::RichText::new(count).small().weak())
                        .selectable(false),
                );
            }
        },
    );
    let answer = ui
        .interact(rect, ui.id().with(id), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    (answer, rect)
}

/// The disclosure mark: pointing right while folded, down while open
///
/// Painted rather than lettered, the faces the chrome is lettered in having
/// no triangle a reader could count on.
fn chevron_mark(ui: &Ui, center: egui::Pos2, open: bool) {
    let arm = 3_f32;
    let (a, b, c) = if open {
        (
            center + egui::vec2(-arm, -arm / 2.),
            center + egui::vec2(0., arm / 2.),
            center + egui::vec2(arm, -arm / 2.),
        )
    } else {
        (
            center + egui::vec2(-arm / 2., -arm),
            center + egui::vec2(arm / 2., 0.),
            center + egui::vec2(-arm / 2., arm),
        )
    };
    let stroke = egui::Stroke::new(1.2_f32, ui.visuals().weak_text_color());
    ui.painter().line_segment([a, b], stroke);
    ui.painter().line_segment([b, c], stroke);
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
