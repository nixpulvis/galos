//! What the map knows about a system, written out
//!
//! Pointing at a system and picking one out are both answered on the map
//! itself, by a ring and a name. This is the long form of the same answer,
//! and the user asks for it deliberately, from the mark beside the name of
//! whatever is picked out. A panel is kept until they shut it, and as many
//! of them stand open at once as they care to open, so two systems can be
//! read side by side.
//!
//! A panel holds a [`System`] value rather than an entity, for the reason a
//! selection does: a system flown away from is despawned, and a panel opened
//! for it has no reason to go with it.

use crate::Db;
use crate::camera::{MoveCamera, OrbitCamera};
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::filter::{Filter, Filters};
use crate::systems::selection::{Picked, Selection};
use crate::ui::Chose;
use crate::ui::MARGIN;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy_egui::egui::{Context, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use elite_journal::body::{Discovery, Orbit, Spin};
use galos_db::bodies::{Body as DbBody, Surface};
use galos_db::factions::Faction as DbFaction;
use galos_db::stars::Star as DbStar;
use galos_db::systems::Economies;
use std::collections::HashMap;
use std::fmt::Display;

pub fn plugin(app: &mut App) {
    app.init_resource::<Panels>();
    app.init_resource::<FactionNames>();
    app.add_systems(Update, refresh.in_set(MapSet::Present));
    app.add_systems(Update, name_factions.in_set(MapSet::Present));
    app.add_systems(Update, fill_filters.in_set(MapSet::Present));
    // `ui::chrome` concludes at its end whether the pointer is busy with the
    // UI, from every window drawn in the pass so far. Drawn before it, these
    // are counted in the same frame they are shown rather than the next.
    app.add_systems(
        EguiPrimaryContextPass,
        panels.after(crate::ui::lettering).before(crate::ui::chrome),
    );
}

/// How wide a panel stands
///
/// Wide enough for a position and for the longest of the names a field is
/// answered with, so that the two columns do not shift from one system to
/// the next.
///
/// A panel comes out at this taken up to a whole character, the title bar
/// being lettered and filled out to its end.
const WIDTH: f32 = 230.;

/// What the title bar spends on the fold arrow, the close mark and the gaps
/// around them
///
/// The rest of [`WIDTH`] is the title's. Measured rather than worked out, egui
/// laying its own title bar out, and held down by
/// `a_short_title_leaves_the_width_alone`.
const TITLE_MARKS: f32 = 46.;

/// How much of a title a panel has room for, in characters
///
/// A window is at least as wide as its title bar needs, so what a panel is
/// called is what decides how wide it stands. Cut to this, a route's panel
/// keeps the width every other panel has.
///
/// Enough characters to cover the room rather than as many as fit inside
/// it, since the title is padded out to fill the bar. A bar a fraction of a
/// character narrower than what stands under it is a panel that draws itself
/// in the moment it is folded away into the bar, and back out when it opens.
fn titling(ctx: &Context) -> usize {
    crate::ui::covering(ctx, egui::TextStyle::Body, WIDTH - TITLE_MARKS)
}

/// What a panel is called, laid out across its title bar
///
/// Lettered as the panel's own contents are. A title set for a heading stands
/// half again as tall as everything under it, which reads as a title bar
/// borrowed from some other window rather than as the top of this one.
///
/// Cut to the room there is, and then filled out to it with the spaces the cut
/// left over: egui centres a title between the marks either side of it, and a
/// title that fills the bar has nothing left to centre, so it stands at the
/// left where a name is read from.
fn titled(ctx: &Context, title: &str) -> egui::RichText {
    let room = titling(ctx);
    let said = crate::ui::shortened(title, room);
    let spare = room.saturating_sub(said.chars().count());

    egui::RichText::new(format!("{said}{}", " ".repeat(spare)))
        .text_style(egui::TextStyle::Body)
}

/// The window a panel stands in, put where the tiling says
///
/// `at` is where its right hand top corner goes, that being the corner the
/// tiling works from: panels stand against the right edge of the viewport, and
/// a window is at least as wide as its title bar needs, so where its left edge
/// falls is not known until it has been drawn.
///
/// `placed` is [`Panel::placed`]: a panel is put where the tiling says on the
/// frame it opens, and asked for `at` as a default after that, so that one
/// dragged somewhere stays where it was dragged.
///
/// The title is cut to the room there is for it, both ends of a route kept.
/// A panel as wide as its name is a panel the tiling cannot place and the
/// user cannot read two of side by side.
fn framed<'open>(
    ctx: &Context,
    title: &str,
    id: egui::Id,
    at: egui::Pos2,
    placed: bool,
    showing: &'open mut bool,
) -> egui::Window<'open> {
    let window = egui::Window::new(titled(ctx, title))
        .id(id)
        .open(showing)
        .resizable(false)
        .pivot(egui::Align2::RIGHT_TOP)
        // The width alone. Left unsaid it is `Style::default_area_size`, 600,
        // which will not fit where a panel is asked to be placed, so egui
        // slides the window somewhere it does and remembers it there. The
        // height is the window's own business: said here it is imposed rather
        // than defaulted, and a list asked to fit the height of the last panel
        // drawn shows three lines of eight.
        .default_width(WIDTH);

    if placed { window.default_pos(at) } else { window.current_pos(at) }
}

/// Lay a panel's contents out over the whole of the window
///
/// [`WIDTH`] is what a panel asks for, and a window is at least as wide as its
/// title bar needs. A route is titled with the names of both its ends, which
/// is wider, and egui hands the extra room to the contents to use or leave.
///
/// They take it. A line in a list is a control the width of the list, so a
/// list laid out to [`WIDTH`] inside a wider window stops short of the frame
/// and leaves a band of empty panel down the right hand side, which reads as
/// a margin nobody chose.
fn spread(ui: &mut Ui) {
    ui.set_min_width(WIDTH);
}

/// What the user has a panel open for
///
/// A list rather than a map, since there are only ever a handful of them and
/// what matters about the order is which places in the tiling are free.
#[derive(Resource, Default)]
pub struct Panels {
    open: Vec<Panel>,
    /// How tall the tallest panel drawn came out
    ///
    /// How far down the next one opens. The tallest rather than the last,
    /// since a system's panel and a filter's are not the same height and a
    /// step measured from the shorter would open the next panel on top of a
    /// taller one already standing.
    ///
    /// Measured rather than guessed because a guess one line short is exactly
    /// the overlap the tiling is for. Nothing until a panel has been drawn,
    /// which leaves the first of a session laid out as taking no room. It
    /// still opens where it asked to, since egui only moves a window to bring
    /// it back inside the viewport.
    height: f32,
    /// How wide the widest panel drawn came out
    ///
    /// How far across the next column of them opens, and the widest for the
    /// reason the height is the tallest. A panel is at least [`WIDTH`] and as
    /// much wider as its title bar needs, which for a route is the names of
    /// both its ends, so a column stepped by what a panel asks for would open
    /// the next one over the top of a wide one already standing.
    ///
    /// Only the columns. Where a panel opens down the edge does not depend on
    /// this, panels being placed by the corner they are tiled against.
    width: f32,
}

/// One open panel
struct Panel {
    /// What it is about
    subject: Subject,
    /// Which place in the tiling it stands in
    slot: usize,
    /// Whether it has been drawn since it was opened
    ///
    /// A panel is put where the tiling says on the frame it opens, and left
    /// wherever it is after that, so that dragging one somewhere holds. Egui
    /// goes on remembering where a window was for the whole session, shut
    /// windows included, so a panel opened a second time would otherwise come
    /// back to the place it had rather than to the place it was just given.
    placed: bool,
}

/// What a panel is about
///
/// Two kinds, sharing the tiling and the window because they are the same
/// gesture answered: the mark on a row in the bar, opening the long form of
/// what that row names.
enum Subject {
    /// One system, and everything the map knows about it
    System(System),
    /// One star, and everything the map knows about it
    ///
    /// The row rather than the entity, for the reason a system's panel holds
    /// one: a star is despawned the moment the camera leaves the system it is
    /// in, and a panel opened for it has no reason to go with it.
    Star(DbStar),
    /// One body, likewise
    Body(DbBody),
    /// One filter, and the systems it admits
    ///
    /// The systems are fetched once the panel is open, by [`fill_filters`],
    /// and are nothing at all until they arrive. They come from the database
    /// rather than from the map, since the point of the list is to say where
    /// a faction is, and the map holds only what the spyglass has reached.
    Filter { filter: Filter, systems: Option<Vec<System>> },
}

impl Subject {
    /// What the panel is titled
    fn title(&self) -> &str {
        match self {
            Subject::System(system) => &system.name,
            Subject::Star(star) => &star.name,
            Subject::Body(body) => &body.name,
            Subject::Filter { filter, .. } => filter.name(),
        }
    }

    /// The identity egui remembers a panel's place by
    ///
    /// Not the title. A system's row is replaced by every fetch that covers
    /// it, so a window named for the row would forget where it was dragged
    /// to; what makes two panels the same panel is what they are about.
    fn id(&self) -> egui::Id {
        match self {
            Subject::System(system) => {
                egui::Id::new(("system-panel", system.address))
            }
            // Which system as well as which of its numbering, an id being
            // one system's own and every system having a body one.
            Subject::Star(star) => {
                egui::Id::new(("star-panel", star.system_address, star.id))
            }
            Subject::Body(body) => {
                egui::Id::new(("body-panel", body.system_address, body.id))
            }
            Subject::Filter { filter, .. } => {
                egui::Id::new(("filter-panel", filter))
            }
        }
    }
}

impl Panels {
    /// Open a panel describing `system`
    ///
    /// A system already being read about is left where it is rather than
    /// opened a second time, since two windows describing one system are two
    /// copies of one answer.
    pub fn open_system(&mut self, system: System) {
        self.push(Subject::System(system));
    }

    /// Open a panel describing `star`
    pub fn open_star(&mut self, star: DbStar) {
        self.push(Subject::Star(star));
    }

    /// Open a panel describing `body`
    pub fn open_body(&mut self, body: DbBody) {
        self.push(Subject::Body(body));
    }

    /// Open a panel listing what `filter` admits
    pub fn open_filter(&mut self, filter: Filter) {
        self.push(Subject::Filter { filter, systems: None });
    }

    fn push(&mut self, subject: Subject) {
        if self.open.iter().any(|panel| panel.subject.id() == subject.id()) {
            return;
        }
        let slot = self.free();
        self.open.push(Panel { subject, slot, placed: false });
    }

    /// The first place in the tiling nothing stands in
    ///
    /// So a panel shut hands its place on, and the next one opened lands in
    /// the gap rather than below everything the user has already read and put
    /// away.
    fn free(&self) -> usize {
        // One more place than there are panels open, so one of them is free
        // however the rest are spread out.
        (0..=self.open.len())
            .find(|slot| self.open.iter().all(|panel| panel.slot != *slot))
            .expect("more places than panels")
    }
}

/// Which place down and across the tiling the panel at `slot` stands in
///
/// Down the right hand edge until another would not fit above the bottom of
/// the viewport, then across into a fresh column to its left, and back to the
/// corner once the viewport is full. Answers in places rather than in pixels,
/// so how large a panel is stays the caller's business.
///
/// Filling the last place is the one time two panels are left on top of each
/// other. There is nowhere else for the next one to go, and shrinking every
/// panel to make room would be a poor trade for the one the user is reading.
fn tile(slot: usize, down: usize, across: usize) -> (usize, usize) {
    let down = down.max(1);
    let slot = slot % (down * across.max(1));
    (slot % down, slot / down)
}

/// What each faction the map has had to name is called
///
/// A [`System`] carries the ids of the factions present in it, since that is
/// what is asked of it in bulk: which of them a filter admits, over every
/// system drawn, every frame. What they are called is asked for rarely and a
/// panel at a time, so it is looked up when a panel wants it and kept.
///
/// Kept for the session. A faction's name does not change, and there are only
/// as many of them here as the user has opened panels for.
#[derive(Resource, Default)]
pub struct FactionNames(HashMap<i32, String>);

impl FactionNames {
    /// What the faction with `id` is called, if it has been looked up
    pub fn get(&self, id: i32) -> Option<&str> {
        self.0.get(&id).map(String::as_str)
    }
}

/// Look up the names of any factions an open panel cannot name yet
///
/// One query for everything unnamed across every open panel, and none at all
/// once they are named, which is every frame but the one after a panel opens.
///
/// Asked for and waited on, as a search is. This is the answer to something
/// the user just did, it is a primary key lookup over a handful of ids, and a
/// panel that filled itself in a moment later would be a panel that looked
/// broken when it opened.
fn name_factions(
    mut names: ResMut<FactionNames>,
    panels: Res<Panels>,
    db: Res<Db>,
) {
    let wanted: Vec<i32> = panels
        .open
        .iter()
        .filter_map(|panel| match &panel.subject {
            Subject::System(system) => Some(system),
            // A filter panel lists systems by name and says nothing about
            // whose they are, and nothing inside a system belongs to anyone:
            // a faction holds a system, not a rock in one.
            Subject::Star(_) | Subject::Body(_) | Subject::Filter { .. } => {
                None
            }
        })
        .flat_map(|system| system.factions.iter().copied())
        .filter(|id| names.get(*id).is_none())
        .collect();
    if wanted.is_empty() {
        return;
    }

    future::block_on(async {
        match DbFaction::fetch_many(&db.0, &wanted).await {
            Ok(factions) => {
                for faction in factions {
                    names.0.insert(faction.id, faction.name);
                }
            }
            // Nothing to be done about it, and nothing to say to the user
            // about a name they did not ask for. The panel says the faction
            // is there and leaves it unnamed.
            Err(why) => debug!("could not name factions: {why}"),
        }
    });
}

/// Keep each panel on whatever the map last heard about its system
///
/// A panel is drawn from the row it was opened with, and a fetch replaces
/// the row of a system already on the map without the panel hearing of it.
/// So a row that has changed is copied across, and a panel says what the map
/// holds rather than what it held.
fn refresh(
    mut panels: ResMut<Panels>,
    changed: Query<&System, Changed<System>>,
) {
    // Every star arrives changed, so without this the whole of a fetch is
    // walked for the sake of the panels nobody has open.
    if panels.open.is_empty() {
        return;
    }
    for system in &changed {
        for panel in &mut panels.open {
            // Only a panel about that one system. A filter's list came from
            // the database rather than the map, and a row that changed under
            // the map says nothing about whether the list is still the right
            // list.
            if let Subject::System(shown) = &mut panel.subject
                && shown.address == system.address
            {
                *shown = system.clone();
            }
        }
    }
}

/// Fetch the systems a filter admits, once a panel has been opened for it
///
/// From the database rather than from the map. The list is there to say where
/// a faction is, and the map holds only what the spyglass has dragged in,
/// which is mostly wherever the user has already been.
///
/// Asked for and waited on, as a search is: this answers something the user
/// just did, and a panel that filled itself in some frames later would look
/// broken when it opened.
///
/// It is the heaviest thing the map waits on. Measured at 44ms for the
/// largest faction on record, which stands in 314 systems, so opening one of
/// those panels drops a couple of frames. Worth moving onto a task if it comes
/// to be done often, and not worth the machinery while it is a click.
fn fill_filters(mut panels: ResMut<Panels>, db: Res<Db>) {
    let unfilled: Vec<Filter> = panels
        .open
        .iter()
        .filter_map(|panel| match &panel.subject {
            Subject::Filter { filter, systems: None } => Some(filter.clone()),
            _ => None,
        })
        .collect();

    for filter in unfilled {
        let found = future::block_on(async { fetch(&db.0, &filter).await });
        for panel in &mut panels.open {
            if let Subject::Filter { filter: shown, systems } =
                &mut panel.subject
                && *shown == filter
            {
                *systems = Some(found.clone());
            }
        }
    }
}

/// What the map can draw of everything `filter` admits
///
/// Put in the filter's own order where it has one, and by name where it has
/// not. What comes back is in no order at all, and which order a list holds
/// is the filter's to say.
///
/// Systems with no position on record are dropped. The map cannot draw one
/// and cannot fly to one, so a line naming it would answer nothing.
///
/// Which systems those are is the filter's own business. This is only what a
/// panel can do with them.
async fn fetch(db: &galos_db::Database, filter: &Filter) -> Vec<System> {
    let rows = filter.systems(db).await;

    let mut found: Vec<System> =
        rows.iter().filter_map(|row| System::try_from(row).ok()).collect();

    // In the filter's own order where it has one, which for a route is the
    // order it is travelled. Where it has none, by name: what comes back is
    // in no order at all, and a list has to be in some order to hold still.
    if filter.ordered() {
        found.sort_by_key(|system| {
            filter.place_of(system.address).unwrap_or(usize::MAX)
        });
    } else {
        found.sort_unstable_by(|one, other| one.name.cmp(&other.name));
    }
    found
}

/// Tell the user what is known about the systems they have opened
///
/// Written here rather than alongside the rest of the UI because a
/// [`System`]'s fields are the business of this module and its neighbours,
/// and this is the one place they are read out rather than drawn with.
fn panels(
    mut contexts: EguiContexts,
    mut panels: ResMut<Panels>,
    names: Res<FactionNames>,
    mut selection: ResMut<Selection>,
    mut filters: ResMut<Filters>,
    mut selected: ResMut<crate::systems::route::Selected>,
    orbit: Query<&OrbitCamera>,
    mut camera: MessageWriter<MoveCamera>,
) -> Result {
    if panels.open.is_empty() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    // Where the camera is looking, which is the distance the spyglass and
    // the selection's own row are measured in.
    let center = orbit.single().map(|camera| camera.center).ok();
    // The top right corner, clear of the settings pane and the bar, which
    // stand against the left edge and the top of it. The corner itself, since
    // a panel is placed by its own right hand top rather than by its left: a
    // window is as wide as its title bar needs, and a place worked out from a
    // width guessed for it is a place a wider panel overhangs the viewport
    // from, to be pushed back inside without the margin it was given.
    //
    // Only where a panel opens: the windows are movable, so where they end
    // up is the user's business.
    let room = ctx.content_rect();
    let corner = room.right_top() + egui::vec2(-MARGIN, MARGIN);

    // A panel and the gap under it, and how many of those the viewport holds
    // each way. Worked out afresh every frame, since the window it is all
    // measured against is the user's to resize.
    let width = panels.width.max(
        WIDTH + egui::Frame::window(&ctx.global_style()).total_margin().sum().x,
    );
    let step = egui::vec2(-(width + MARGIN), panels.height + MARGIN);
    let down = ((room.height() - MARGIN) / step.y).floor().max(1.) as usize;
    let across = ((room.width() - MARGIN) / -step.x).floor().max(1.) as usize;

    let mut shut = Vec::new();
    let mut tallest: f32 = 0.;
    let mut widest: f32 = 0.;
    let mut centered = None;
    let mut picked = None;
    let mut opening = None;
    let mut wanted = None;
    // Which panel the user pressed, if they pressed one. Asked once for the
    // whole pass, since a press is one press however many windows are drawn.
    let pressed = ctx.input(|input| input.pointer.any_pressed());
    let mut chosen = None;
    for panel in &mut panels.open {
        let mut showing = true;
        let (row, column) = tile(panel.slot, down, across);
        let at =
            corner + egui::vec2(step.x * column as f32, step.y * row as f32);
        // Set before the panel is read from, so that the two borrows of it
        // do not overlap.
        let placed = std::mem::replace(&mut panel.placed, true);
        let window = framed(
            ctx,
            panel.subject.title(),
            panel.subject.id(),
            at,
            placed,
            &mut showing,
        );
        let window = window.show(ctx, |ui| {
            spread(ui);
            match &panel.subject {
                Subject::System(system) => {
                    described(ui, system, &names, &mut centered, &mut wanted)
                }
                Subject::Star(star) => star_described(ui, star),
                Subject::Body(body) => body_described(ui, body),
                Subject::Filter { filter, systems } => admitted(
                    ui,
                    filter,
                    systems.as_deref(),
                    center,
                    &mut picked,
                    &mut opening,
                    &mut centered,
                ),
            }
        });

        // Only a panel that drew what it holds. A window rolled up into its
        // title bar stands a line high, which is no height to place the next
        // panel by. Its width is what it always was, the title bar being the
        // one part of it that is drawn either way, so that is taken from any
        // panel that was shown at all.
        if let Some(window) = window {
            widest = widest.max(window.response.rect.width());
            if window.inner.is_some() {
                tallest = tallest.max(window.response.rect.height());
            }
            // A press landing on a panel is how the user says which of them
            // they are working with, and egui has already settled which
            // window that press reached: `contains_pointer` answers for the
            // one on top, so a panel under another does not take a press
            // meant for it.
            if pressed
                && window.response.contains_pointer()
                && let Subject::Filter { filter, .. } = &panel.subject
            {
                chosen = Some(filter.clone());
            }
        }
        if !showing {
            shut.push(panel.subject.id());
        }
    }

    if let Some(position) = centered {
        camera.write(MoveCamera { position: Some(position), framing: None });
    }
    // Picking a system out of a list says which one is meant, as clicking a
    // star does. Where the camera goes is asked for separately, from the row
    // in the bar that names what is picked out.
    if let Some((system, gathering)) = picked {
        selection.pick(Picked::System(system), gathering);
    }
    // Opened after the loop, since a panel asked for from inside one is a
    // panel pushed onto the list being walked.
    if let Some(system) = opening {
        panels.open_system(system);
    }
    // Already resolved, both halves of it having been read off a system the
    // map holds, so it goes straight in rather than round by `Wanted`.
    if let Some(filter) = wanted {
        filters.add(filter);
    }
    // Pressing a route's panel is how the user says which of several drawn
    // routes they mean, and the map draws that one in front of the rest.
    // Only a route: the other filters have no line to put forward, and a
    // press on one of their panels says nothing about which route is which.
    if let Some(filter) = chosen
        && filter.is_route()
    {
        selected.0 = Some(filter);
    }

    // Nothing while every panel is rolled up into its title bar, which is
    // not a height to place the next one by.
    if tallest > 0. {
        panels.height = tallest;
    }
    if widest > 0. {
        panels.width = widest;
    }
    if !shut.is_empty() {
        panels.open.retain(|panel| !shut.contains(&panel.subject.id()));
    }

    Ok(())
}

/// Everything the map knows about one system
fn described(
    ui: &mut Ui,
    system: &System,
    names: &FactionNames,
    centered: &mut Option<DVec3>,
    wanted: &mut Option<Filter>,
) {
    egui::Grid::new(("system-fields", system.address)).num_columns(2).show(
        ui,
        |ui| {
            let [x, y, z] = system.position;
            field(ui, "Position", format!("{x:.2}, {y:.2}, {z:.2}"));
            field(ui, "Population", crate::ui::thousands(system.population));
            field(ui, "Allegiance", named(&system.allegiance));
            field(ui, "Government", named(&system.government));
            field(ui, "Security", named(&system.security));
            economies(ui, &system.economies);
            field(
                ui,
                "Updated",
                system.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
        },
    );

    factions(ui, &system.factions, names, wanted);

    // Its own system rather than whatever is selected, since several panels
    // stand open at once and each one is about the system named in its title
    // bar.
    ui.add_space(MARGIN);
    if ui.button("Center Camera").clicked() {
        *centered = Some(DVec3::from(system.position));
    }
}

/// Metres in a solar radius
///
/// What a star's size is read in. Metres are what the database holds and what
/// the map draws in, and a star written out in them is a run of digits nobody
/// counts.
const SOLAR_RADIUS: f64 = 6.957e8;

/// Metres per second squared in a gravity
///
/// The journal records what a body pulls at in metres per second squared, and
/// what anybody wants to know is how that compares to standing on Earth.
const GRAVITY: f64 = 9.80665;

/// Pascals in an atmosphere
const ATMOSPHERE: f64 = 101_325.;

/// Seconds in a day
const DAY: f64 = 86_400.;

/// Everything the map knows about one star
///
/// Read in the units a star is talked about in rather than the ones it is
/// stored in: suns for its size and its mass, days for its turn, and light
/// seconds for how far out it stands.
fn star_described(ui: &mut Ui, star: &DbStar) {
    egui::Grid::new(("star-fields", star.system_address, star.id))
        .num_columns(2)
        .show(ui, |ui| {
            field(ui, "Class", format!("{}{}", star.star_class, star.subclass));
            field(ui, "Luminosity", star.luminosity.clone());
            field(
                ui,
                "Distance",
                format!("{:.1} Ls", star.distance_from_arrival_ls),
            );
            field(
                ui,
                "Radius",
                format!("{:.3} Sol", star.radius as f64 / SOLAR_RADIUS),
            );
            field(ui, "Mass", format!("{:.3} Sol", star.stellar_mass));
            field(ui, "Temperature", format!("{:.0} K", star.temperature));
            field(
                ui,
                "Age",
                format!(
                    "{} million years",
                    crate::ui::thousands(star.age_my.max(0) as u64)
                ),
            );
            field(ui, "Magnitude", format!("{:.2}", star.absolute_magnitude));
            turning(ui, &star.spin);
            circling(ui, star.orbit.as_ref());
            found(ui, &star.discovery);
            field(
                ui,
                "Updated",
                star.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
        });
}

/// Everything the map knows about one body
fn body_described(ui: &mut Ui, body: &DbBody) {
    egui::Grid::new(("body-fields", body.system_address, body.id))
        .num_columns(2)
        .show(ui, |ui| {
            field(ui, "Class", body.planet_class.clone());
            field(
                ui,
                "Distance",
                match body.distance_from_arrival {
                    Some(away) => format!("{away:.1} Ls"),
                    None => UNKNOWN.into(),
                },
            );
            field(
                ui,
                "Radius",
                format!(
                    "{} km",
                    crate::ui::thousands((body.radius / 1e3) as u64)
                ),
            );
            field(ui, "Mass", format!("{:.3} Earths", body.mass));
            field(
                ui,
                "Gravity",
                format!("{:.2} g", body.gravity as f64 / GRAVITY),
            );
            field(ui, "Temperature", format!("{:.0} K", body.temperature));
            standing(ui, &body.surface);
            field(ui, "Tidal lock", yes(body.tidal_lock));
            turning(ui, &body.spin);
            circling(ui, Some(&body.orbit));
            found(ui, &body.discovery);
            field(
                ui,
                "Updated",
                body.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
        });
}

/// What a body's surface is like, where it has one
///
/// A gas giant has none, and is one line saying so rather than a header over
/// five rows of nothing.
fn standing(ui: &mut Ui, surface: &Option<Surface>) {
    let Some(surface) = surface else {
        field(ui, "Surface", "None".into());
        return;
    };

    ui.label(egui::RichText::new("Surface").strong());
    ui.end_row();
    under(ui, "Landable", yes(surface.landable));
    under(ui, "Atmosphere", surface.atmosphere_type.to_string());
    under(
        ui,
        "Pressure",
        format!("{:.3} atm", surface.pressure as f64 / ATMOSPHERE),
    );
    under(ui, "Volcanism", named(&surface.volcanism));
    under(ui, "Terraforming", named(&surface.terraform_state));
}

/// How a thing turns on its own axis
fn turning(ui: &mut Ui, spin: &Spin) {
    ui.label(egui::RichText::new("Spin").strong());
    ui.end_row();
    under(ui, "Period", spent(spin.period));
    under(ui, "Tilt", format!("{:.1}°", spin.tilt.to_degrees()));
}

/// The path a thing takes about whatever it goes round
///
/// Nothing to say for the one that goes round nothing, which is the star a
/// system arrives at.
fn circling(ui: &mut Ui, orbit: Option<&Orbit>) {
    let Some(orbit) = orbit else {
        field(ui, "Orbit", "Goes round nothing".into());
        return;
    };

    ui.label(egui::RichText::new("Orbit").strong());
    ui.end_row();
    under(ui, "Radius", spanning(orbit.semi_major_axis));
    under(ui, "Period", spent(orbit.orbital_period));
    under(ui, "Eccentricity", format!("{:.4}", orbit.eccentricity));
    under(ui, "Inclination", format!("{:.1}°", orbit.orbital_inclination));
}

/// What is known about a thing rather than about the thing itself
fn found(ui: &mut Ui, discovery: &Discovery) {
    ui.label(egui::RichText::new("Discovery").strong());
    ui.end_row();
    under(ui, "Discovered", yes(discovery.discovered));
    under(ui, "Mapped", yes(discovery.mapped));
}

/// How long something takes, in the largest unit it fills
///
/// Days for anything that turns slowly, which is most of what is scanned, and
/// hours for the rest. A period in seconds is eight digits nobody reads.
fn spent(seconds: f32) -> String {
    if seconds <= 0. {
        return UNKNOWN.into();
    }
    let days = seconds as f64 / DAY;
    if days >= 1. {
        format!("{days:.1} days")
    } else {
        format!("{:.1} hours", days * 24.)
    }
}

/// How far something reaches, in the larger unit it fills
///
/// Light seconds where a light second is not most of the answer, and
/// kilometres where it is. A moon a few thousand kilometres out is a
/// hundredth of a light second, and a planet is millions of kilometres.
fn spanning(metres: f32) -> String {
    let metres = metres as f64;
    if metres >= crate::space::LIGHT_SECOND {
        format!("{:.2} Ls", metres / crate::space::LIGHT_SECOND)
    } else {
        format!("{} km", crate::ui::thousands((metres / 1e3) as u64))
    }
}

/// What the database says of a yes or no question
fn yes(answer: bool) -> String {
    if answer { "Yes".into() } else { "No".into() }
}

/// How many systems a filter's panel lists before it starts scrolling
///
/// Enough to read a faction's holdings at a glance, and few enough that a
/// panel does not run the height of the viewport and leave the tiling
/// nowhere to put the next one.
const LISTED: usize = 8;

/// What a filter's panel says it is showing, above the list of it
///
/// How many systems, and for a route the range it was plotted for as well. A
/// panel is titled with what the filter is called, and a route is called after
/// its two ends, so two plots between the same pair come up under the same
/// name. What tells them apart is the range that was asked for, and how many
/// systems that came to: a ship that reaches further crosses the same gap in
/// fewer.
///
/// A range rather than a jump. It is how far the ship can go in one, which is
/// what the route was worked out against; how far it actually goes is the
/// distance on each line of the list below, and is usually less.
///
/// Said in the panel rather than in the title, which is cut to the room a
/// window has and would lose it. The other filters are named for the whole of
/// what they are and have nothing to add here.
fn summary(filter: &Filter, held: usize) -> String {
    match filter.range() {
        Some(range) => format!("{held} systems, {range} Ly range"),
        None => format!("{held} systems"),
    }
}

/// The systems a filter admits, and the one the user picks out of them
///
/// Every system the database has for the filter, not only the ones the map
/// has fetched, since where a faction is, is most of what is being asked.
///
/// Answered the way the map itself is: one click says which system is meant
/// and a second says to go there, so a system reached through a list and a
/// system reached by its star are reached the same way.
///
/// Each line ends in a distance, and which distance it is follows from what
/// the list is: the jump that reaches the system where the filter is flown,
/// and how far off it is from the camera where it is not.
fn admitted(
    ui: &mut Ui,
    filter: &Filter,
    systems: Option<&[System]>,
    center: Option<DVec3>,
    picked: &mut Option<(System, bool)>,
    described: &mut Option<System>,
    centered: &mut Option<DVec3>,
) {
    let Some(systems) = systems else {
        ui.label(egui::RichText::new("Looking...").weak());
        return;
    };

    if systems.is_empty() {
        ui.label(egui::RichText::new("No systems on record").weak());
        return;
    }

    ui.label(egui::RichText::new(summary(filter, systems.len())).weak());
    ui.add_space(MARGIN);

    let line = ui.text_style_height(&egui::TextStyle::Body)
        + crate::ui::LINE_PADDING * 2.
        + ui.spacing().item_spacing.y;
    // What each line has to say about where its system is, which is not the
    // same question in the two kinds of list.
    //
    // A route is flown, so what is worth knowing about a system on one is the
    // jump that reaches it: how far it is from the system before, which is
    // what a ship has to be able to make. Measured off the list, which for a
    // route is already in the order it is travelled. The first system is
    // where the flying starts and no jump reaches it, so its line says
    // nothing.
    //
    // Everywhere else the systems are a set, in no order but the one this
    // list puts them in, and how far off they are from where the camera is
    // looking is both what orders them and what says why.
    let mut order: Vec<(&System, Option<f64>)> = if filter.ordered() {
        let mut legs = Vec::with_capacity(systems.len());
        let mut left = None;
        for system in systems {
            let at = DVec3::from(system.position);
            legs.push((system, left.map(|from: DVec3| from.distance(at))));
            left = Some(at);
        }
        legs
    } else {
        systems
            .iter()
            .map(|system| {
                (
                    system,
                    center.map(|at| at.distance(DVec3::from(system.position))),
                )
            })
            .collect()
    };

    // A filter with an order of its own is left in it. A route is travelled
    // from one end to the other, and a list of its systems put in any other
    // order is no longer a route, whatever it is sorted by.
    //
    // Where there is no such order, nearest first, from where the camera is
    // looking, which is the distance the whole map is measured in. Ordered
    // afresh each frame rather than once when the list arrived, so it goes on
    // answering which of these is near me as the user flies. That does mean
    // it reorders while the camera is moving; it holds still the moment it
    // stops, and the camera only moves when it is asked to.
    //
    // A stable sort, so that with no camera to measure from the order the
    // list arrived in is what is left.
    if !filter.ordered() {
        order.sort_by(|(_, one), (_, other)| match (one, other) {
            (Some(one), Some(other)) => one.total_cmp(other),
            _ => std::cmp::Ordering::Equal,
        });
    }

    // Named for the filter it lists, since a panel stands per filter and two
    // of them open at once are two lists, each scrolled to its own place.
    crate::ui::scrolling(ui, line * LISTED as f32, filter, |ui| {
        for (index, (system, away)) in order.into_iter().enumerate() {
            // Said as well as sorted by, where it is what sorts them. A list
            // in an order nobody can see reads as an order nobody chose.
            let trailing = away.map(|away| format!("{away:.1} Ly"));
            // Keyed by place rather than by which system stands there. The
            // list is put in order afresh every frame, so a row holds its
            // rectangle while the system in it changes as the camera moves,
            // which is the one thing egui reads as a widget taking another's
            // state.
            let asked = crate::ui::system_line(
                ui,
                &system.name,
                trailing,
                true,
                ("admitted", index),
            );
            match asked {
                Some(Chose::Select { gathering }) => {
                    *picked = Some((system.clone(), gathering))
                }
                Some(Chose::Travel) => {
                    *centered = Some(DVec3::from(system.position))
                }
                Some(Chose::Describe) => *described = Some(system.clone()),
                None => {}
            }
        }
    });
}

/// Every faction present in the system, one to a line
///
/// A list under a heading rather than rows in the grid above, because a
/// system holds as many factions as it holds and their names run long. Set
/// against the grid's two columns each would wrap into a paragraph, and the
/// column widths the rest of the panel is laid out in would be decided by
/// whichever faction happened to have the longest name.
///
/// Sorted by name, so that the order holds still. What comes back from the
/// database is in no order at all, and a fetch that replaced the row would
/// otherwise shuffle the list under whoever was reading it.
///
/// A faction whose name has not arrived yet is still one of the factions
/// here, so it keeps its line. Naming them is one query behind the panel
/// opening, and a list that grew a line a frame later would jump.
///
/// Clicking one asks the map for it: the faction becomes a filter, and
/// everything it is absent from goes dim. A system's panel is where the user
/// finds out who is here, and where else they are is the next question, so it
/// is asked from the answer rather than typed out again in the bar.
///
/// A faction still waiting on its name does not answer. What it is called is
/// half of a filter, being what its row in the bar says it is, and a row
/// naming a faction as punctuation would say nothing about what had gone dim.
fn factions(
    ui: &mut Ui,
    present: &[i32],
    names: &FactionNames,
    wanted: &mut Option<Filter>,
) {
    if present.is_empty() {
        return;
    }

    ui.add_space(MARGIN);
    ui.label(egui::RichText::new("Factions").strong());
    for (id, name) in listed(present, names) {
        let text = match name {
            Some(name) => egui::RichText::new(name),
            None => egui::RichText::new(UNNAMED).weak(),
        };
        let (_, answer) = crate::ui::line(ui, text, 0., name.is_some());

        if let Some(name) = name
            && answer.clicked()
        {
            *wanted = Some(Filter::Faction { id, name: name.to_owned() });
        }
    }
}

/// The factions of a system as their lines read, in order
///
/// Named first and among themselves by name, with whatever is still unnamed
/// held at the end. Sorting the placeholder in with the names would put it
/// above all of them, since it is punctuation, and the line would then jump
/// the length of the list the moment its name arrived.
///
/// The id rides along with the name because a line is a control: clicking one
/// asks for a filter, and a filter tests a system against the id.
fn listed<'a>(
    present: &[i32],
    names: &'a FactionNames,
) -> Vec<(i32, Option<&'a str>)> {
    let mut listed: Vec<(i32, Option<&str>)> =
        present.iter().map(|id| (*id, names.get(*id))).collect();
    listed.sort_unstable_by_key(|(id, name)| (name.is_none(), *name, *id));
    listed
}

/// A faction the map has yet to hear the name of
const UNNAMED: &str = "...";

/// One named thing the database knows about a system
///
/// The name is written as brightly as a header, and the answer beside it in
/// the ordinary text of the panel. A panel is read down its left hand column
/// until the line wanted is found, and it is the names that column is made
/// of.
fn field(ui: &mut Ui, name: &str, value: String) {
    ui.label(egui::RichText::new(name).strong());
    ui.label(value);
    ui.end_row();
}

/// How far a field written under a header sits in from the ones above it
const NEST: f32 = 12.;

/// One named thing, written under the header it belongs to
fn under(ui: &mut Ui, name: &str, value: String) {
    ui.horizontal(|ui| {
        ui.add_space(NEST);
        ui.label(egui::RichText::new(name).strong());
    });
    ui.label(value);
    ui.end_row();
}

/// What a system trades in
///
/// Under a header, because the halves of an economy are named for their
/// standing against each other: "Secondary" among the flat fields says what
/// it is but not what it is secondary to.
///
/// A system with nothing on record is one line saying so. There is no primary
/// to head the pair with, and two nested lines both reading `UNKNOWN` say the
/// same thing at three times the length.
fn economies(ui: &mut Ui, economies: &Option<Economies>) {
    let Some(economies) = economies else {
        field(ui, "Economy", UNKNOWN.into());
        return;
    };

    ui.label(egui::RichText::new("Economy").strong());
    ui.end_row();
    under(ui, "Primary", economies.primary.to_string());
    under(ui, "Secondary", named(&economies.secondary));
}

/// What the database has yet to say about a system
const UNKNOWN: &str = "Unknown";

/// What the database says, or that it says nothing
///
/// Most of what is recorded about a system is optional, and a blank row
/// reads as a bug rather than as an answer.
fn named<T: Display>(value: &Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => UNKNOWN.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::system;
    use crate::tests::{context, painted, words};
    use chrono::DateTime;
    use elite_journal::Allegiance;
    use elite_journal::body::{
        Discovery as JournalDiscovery, Orbit as JournalOrbit,
        Spin as JournalSpin,
    };
    use elite_journal::system::Economy;

    /// A registry naming each of `known`
    fn known(known: &[(i32, &str)]) -> FactionNames {
        FactionNames(
            known.iter().map(|(id, name)| (*id, name.to_string())).collect(),
        )
    }

    /// The color each piece of text `contents` painted came out in
    ///
    /// Read back off the shapes for the reason [`crate::tests::words`] is:
    /// a panel is read by what its lines say and how brightly they say it,
    /// and the widget that wrote a line is gone by the time it is painted.
    ///
    /// A color asked for when the text was laid out is the one it keeps.
    /// Text laid out without one carries a placeholder instead, and what
    /// answers that is whatever the shape was painted with.
    fn written(
        mut contents: impl FnMut(&mut Ui),
    ) -> Vec<(String, egui::Color32)> {
        let ctx = context();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));

        fn text_of(
            shape: &egui::Shape,
            into: &mut Vec<(String, egui::Color32)>,
        ) {
            match shape {
                egui::Shape::Text(text) => {
                    let asked = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.color)
                        .filter(|color| *color != egui::Color32::PLACEHOLDER);
                    let color = text
                        .override_text_color
                        .or(asked)
                        .unwrap_or(text.fallback_color);
                    into.push((text.galley.text().into(), color));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_of(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut painted = Vec::new();
        for shape in &output.shapes {
            text_of(&shape.shape, &mut painted);
        }
        painted
    }

    /// What the economy of a system reads as, line by line
    ///
    /// In a grid, since that is where a panel writes its fields and what
    /// [`under`] indents against.
    fn economy(economies: Option<Economies>) -> Vec<String> {
        words(|ui| {
            egui::Grid::new("economies").num_columns(2).show(ui, |ui| {
                super::economies(ui, &economies);
            });
        })
    }

    /// Both halves are written out, under a header naming what they are
    #[test]
    fn an_economy_reads_as_its_two_halves() {
        let economies = Some(Economies {
            primary: Economy::Agriculture,
            secondary: Some(Economy::Extraction),
        });

        assert_eq!(
            economy(economies),
            vec![
                "Economy",
                "Primary",
                "Agriculture",
                "Secondary",
                "Extraction"
            ]
        );
    }

    /// A system with only a primary is still asked about its secondary
    ///
    /// The header promises two lines, and dropping the one that is unknown
    /// would read as a system whose secondary is something the panel is not
    /// saying.
    #[test]
    fn an_economy_missing_its_secondary_says_so() {
        let economies =
            Some(Economies { primary: Economy::Agriculture, secondary: None });

        assert_eq!(
            economy(economies),
            vec!["Economy", "Primary", "Agriculture", "Secondary", UNKNOWN]
        );
    }

    /// A system with nothing on record says it in one line
    #[test]
    fn a_system_with_no_economy_says_so_once() {
        assert_eq!(economy(None), vec!["Economy", UNKNOWN]);
    }

    /// Founders World, as the database holds it
    fn founders() -> DbBody {
        DbBody {
            system_address: 1,
            id: 14,
            parents: vec![],
            name: "Founders World".to_owned(),
            body_type: None,
            distance_from_arrival: Some(345.5563),
            updated_at: DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            planet_class: "Earthlike body".to_owned(),
            tidal_lock: false,
            mass: 0.69,
            radius: 5.485766e6,
            gravity: 9.13869,
            temperature: 298.70755,
            surface: None,
            orbit: JournalOrbit {
                semi_major_axis: 1.0023064e11,
                eccentricity: 0.0026,
                orbital_inclination: 1.5,
                periapsis: 0.,
                orbital_period: 8.6e7,
                ascending_node: 0.,
                mean_anomaly: 0.,
            },
            spin: JournalSpin { period: 3.6254802e6, tilt: 0.373026 },
            discovery: JournalDiscovery { discovered: false, mapped: true },
        }
    }

    /// What a body's panel says
    fn body_said() -> Vec<String> {
        words(|ui| body_described(ui, &founders()))
    }

    /// A body's panel reads in the units a body is talked about in
    ///
    /// Which are not the ones it is stored in. The database holds metres and
    /// metres per second squared because that is what a scan records and what
    /// the map draws with, and nobody asks how many metres across a world is.
    #[test]
    fn a_body_is_described_in_the_units_it_is_read_in() {
        let said = body_said();

        assert!(said.contains(&"Earthlike body".to_owned()), "{said:?}");
        assert!(said.contains(&"345.6 Ls".to_owned()), "{said:?}");
        assert!(said.contains(&"5,485 km".to_owned()), "{said:?}");
        assert!(said.contains(&"0.690 Earths".to_owned()), "{said:?}");
        assert!(said.contains(&"0.93 g".to_owned()), "{said:?}");
        assert!(said.contains(&"299 K".to_owned()), "{said:?}");
    }

    /// A gas giant has no surface, and says so in one line
    ///
    /// Rather than a header over five rows of nothing, as a system with no
    /// economy on record does.
    #[test]
    fn a_body_with_no_surface_says_so_once() {
        let said = body_said();

        assert!(said.contains(&"Surface".to_owned()), "{said:?}");
        assert!(said.contains(&"None".to_owned()), "{said:?}");
        assert!(!said.contains(&"Landable".to_owned()), "{said:?}");
    }

    /// A body's turn and its orbit are read in days
    ///
    /// A period is recorded in seconds, and eight digits of them is a number
    /// nobody reads.
    #[test]
    fn what_turns_slowly_is_read_in_days() {
        let said = body_said();

        assert!(said.contains(&"42.0 days".to_owned()), "{said:?}");
        assert!(said.contains(&"21.4°".to_owned()), "{said:?}");
    }

    /// How far something reaches is read in whichever unit it fills
    #[test]
    fn a_reach_is_read_in_the_unit_it_fills() {
        assert_eq!(spanning(1.0023064e11), "334.33 Ls");
        assert_eq!(spanning(5.4946205e6), "5,494 km");
    }

    /// And how long it takes, likewise
    #[test]
    fn a_span_of_time_is_read_in_the_unit_it_fills() {
        assert_eq!(spent(3.6254802e6), "42.0 days");
        assert_eq!(spent(3600.), "1.0 hours");
        assert_eq!(spent(0.), UNKNOWN);
    }

    /// The names down the left of a panel are written as brightly as a header
    ///
    /// Nested or flat, a name is what the column is read down, and one in the
    /// same color as the answers beside it leaves the two to be told apart
    /// by which side of the panel they fell on.
    #[test]
    fn the_names_of_fields_read_as_brightly_as_a_header() {
        let painted = written(|ui| {
            egui::Grid::new("fields").num_columns(2).show(ui, |ui| {
                field(ui, "Security", "Low".into());
                economies(
                    ui,
                    &Some(Economies {
                        primary: Economy::Military,
                        secondary: None,
                    }),
                );
            });
        });

        let color = |wanted: &str| {
            painted
                .iter()
                .find(|(text, _)| text == wanted)
                .unwrap_or_else(|| panic!("{wanted} was painted"))
                .1
        };

        assert_eq!(color("Security"), color("Economy"));
        assert_eq!(color("Primary"), color("Economy"));
        assert_ne!(color("Low"), color("Economy"));
        assert_ne!(color("Military"), color("Economy"));
    }

    /// A system with no factions lists none
    #[test]
    fn a_system_with_no_factions_lists_nothing() {
        assert!(listed(&[], &known(&[])).is_empty());
    }

    /// The list reads in name order, whatever order the ids came in
    ///
    /// What the database returns is in no order at all, and a fetch that
    /// replaced the row would otherwise shuffle the list under whoever was
    /// reading it.
    #[test]
    fn factions_are_listed_by_name() {
        let names = known(&[(1, "Zargon Front"), (2, "Alliance of Sol")]);
        let wanted =
            vec![(2, Some("Alliance of Sol")), (1, Some("Zargon Front"))];

        assert_eq!(listed(&[1, 2], &names), wanted);
        assert_eq!(listed(&[2, 1], &names), wanted);
    }

    /// A line comes out in a color something can draw
    ///
    /// A line is laid out from whatever it is handed, which is usually plain
    /// text carrying no color of its own. A placeholder answered by a
    /// placeholder reaches the tessellator, which panics, and takes every
    /// panel holding a list with it: the factions of a system, and the systems
    /// of a filter.
    #[test]
    fn a_line_paints_in_a_color() {
        painted(|ui| {
            crate::ui::line(
                ui,
                egui::RichText::new("Alliance of Sol"),
                0.,
                true,
            );
        });
    }

    /// So does one with room kept at its end
    #[test]
    fn a_line_with_room_reserved_paints_in_a_color() {
        painted(|ui| {
            crate::ui::line(ui, egui::RichText::new("Sol"), 20., true);
        });
    }

    /// And so does a whole faction list, marks and placeholders included
    #[test]
    fn a_faction_list_paints_in_a_color() {
        let names = known(&[(1, "Alliance of Sol")]);
        painted(|ui| {
            // One named and one still waiting, so both arms are drawn.
            factions(ui, &[1, 2], &names, &mut None);
        });
    }

    /// And a filter's list of systems, which carries a distance and a mark
    #[test]
    fn a_system_list_paints_in_a_color() {
        let systems = [system(1), system(2)];
        painted(|ui| {
            admitted(
                ui,
                &faction(7),
                Some(&systems),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
            );
        });
    }

    /// So does one a route lists in the order it is travelled
    #[test]
    fn a_route_list_paints_in_a_color() {
        let systems = [system(1), system(2)];
        let route = Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2],
            range: "10".to_owned(),
        };
        painted(|ui| {
            admitted(
                ui,
                &route,
                Some(&systems),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
            );
        });
    }

    /// The rectangle a panel titled `title` came out in, opened at `at`
    ///
    /// The window as the panels draw it, `contents` being whatever the test
    /// wants to ask of the room inside it.
    fn shown(
        title: &str,
        at: egui::Pos2,
        contents: impl FnMut(&mut Ui),
    ) -> egui::Rect {
        let mut contents = contents;
        let ctx = crate::tests::context();
        let mut rect = egui::Rect::ZERO;

        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut showing = true;
            let panel = framed(
                ui.ctx(),
                title,
                egui::Id::new("test-panel"),
                at,
                false,
                &mut showing,
            );
            if let Some(panel) = panel.show(ui.ctx(), &mut contents) {
                rect = panel.response.rect;
            }
        });

        rect
    }

    /// How wide a panel titled `title` lays its contents out, and how much
    /// room it had
    ///
    /// What is being asked is what the contents make of the width the title
    /// left them.
    fn laid_out(title: &str) -> (f32, f32) {
        let mut taken = 0.;
        let rect = shown(title, egui::Pos2::ZERO, |ui| {
            spread(ui);
            taken = ui.available_width();
        });
        let margins =
            egui::Frame::window(&crate::tests::context().global_style())
                .total_margin()
                .sum()
                .x;

        (taken, rect.width() - margins)
    }

    /// A panel's contents are laid out across the whole of its window
    ///
    /// A window is at least as wide as its title bar, and a system is called
    /// what it is called: it is not cut down the way a route's two ends are,
    /// so a long enough name is a wider panel. Contents laid out to the width
    /// a panel asks for would leave a band of empty panel down the right hand
    /// side of one, which reads as a margin nobody chose.
    ///
    /// Named at a length no lettering would fit, rather than at one that fits
    /// today and not tomorrow.
    #[test]
    fn a_long_title_widens_what_stands_under_it() {
        let (taken, had) = laid_out(&"COL 285 SECTOR ".repeat(4));

        assert!(had > WIDTH, "{had} is no wider than the {WIDTH} asked for");
        assert_eq!(taken, had);
    }

    /// How wide one character of a panel's lettering stands
    fn character() -> f32 {
        let ctx = crate::tests::context();
        // Inside a pass, egui having no fonts to measure with before one.
        let mut one = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            one = crate::ui::one_character(ui.ctx(), egui::TextStyle::Body);
        });

        one
    }

    /// A title that fits leaves the panel the width it asked for
    ///
    /// To the character. A title bar holds a whole number of them and is
    /// filled out to its end, so a panel stands at the width asked for taken
    /// up to the next one, and never short of it.
    ///
    /// Which is also what holds [`TITLE_MARKS`] down. What the title bar
    /// spends on the fold arrow and the close mark is measured off egui
    /// rather than worked out, and a panel standing anywhere but within a
    /// character of the width asked for is that measurement having drifted.
    #[test]
    fn a_short_title_leaves_the_width_alone() {
        let (taken, had) = laid_out("SOL");

        assert_eq!(taken, had);
        assert!(had >= WIDTH, "{had} is narrower than the {WIDTH} asked for");
        assert!(
            had < WIDTH + character(),
            "{had} is over a character wider than the {WIDTH} asked for"
        );
    }

    /// How wide a panel comes out, folded away into its title bar or open
    ///
    /// Drawn over enough passes for egui to have finished animating the
    /// fold, a panel halfway into one being neither width.
    fn width(folded: bool) -> f32 {
        let ctx = crate::tests::context();
        let id = egui::Id::new("test-panel");
        let mut width = 0.;

        for pass in 0..20 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                if pass == 0 {
                    let mut fold =
                        egui::containers::collapsing_header::CollapsingState::
                            load_with_default_open(
                                ui.ctx(),
                                id.with("collapsing"),
                                true,
                            );
                    fold.set_open(!folded);
                    fold.store(ui.ctx());
                }

                let mut showing = true;
                let panel = framed(
                    ui.ctx(),
                    "SOL",
                    id,
                    egui::Pos2::ZERO,
                    pass > 0,
                    &mut showing,
                );
                if let Some(panel) = panel.show(ui.ctx(), |ui| {
                    spread(ui);
                    ui.label("5 systems");
                }) {
                    width = panel.response.rect.width();
                }
            });
        }

        width
    }

    /// Folding a panel away leaves its width alone
    ///
    /// A window is as wide as its title bar needs and as wide as whatever
    /// stands under it, and a folded panel is the title bar alone. A bar
    /// narrower than the contents is a panel that draws itself in the moment
    /// the fold finishes, and back out again when it is opened.
    #[test]
    fn folding_a_panel_leaves_its_width_alone() {
        assert_eq!(width(true), width(false));
    }

    /// A title fills the bar it stands in
    ///
    /// Which is what puts it at the left: egui centres a title between the
    /// marks either side of it, and one that fills the bar has nothing left
    /// to centre.
    #[test]
    fn a_title_fills_the_bar_it_stands_in() {
        let ctx = crate::tests::context();
        // Inside a pass, egui having no fonts to measure with before one.
        let (mut said, mut room) = (String::new(), 0);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            said = titled(ui.ctx(), "SOL").text().to_owned();
            room = titling(ui.ctx());
        });

        assert!(said.starts_with("SOL"), "{said:?}");
        assert_eq!(said.chars().count(), room);
    }

    /// A route's panel is no wider than a panel, however it is named
    ///
    /// Both ends of a route run long, and a window is as wide as its title
    /// bar needs. Left whole, the title decides how wide the panel stands,
    /// which is a panel the tiling cannot step by and the user cannot read
    /// two of side by side.
    #[test]
    fn a_route_panel_is_no_wider_than_a_panel() {
        let (_, route) = laid_out("SIGMA DRACONIS -> MINISTRY");
        let (_, plain) = laid_out("SOL");

        assert_eq!(route, plain);
    }

    /// A panel opens with its right hand top corner where it was put
    ///
    /// Which is what standing off the edge of the viewport by the margin
    /// comes to. Placed by its left edge instead, a panel wider than the
    /// width it was given would reach past that edge, and egui would push it
    /// back inside with nothing between it and the corner.
    ///
    /// Both titles, since what a panel is called is what widens it and the
    /// corner is not to move with the words in the title bar.
    #[test]
    fn a_panel_opens_at_the_corner_it_is_given() {
        let at = egui::pos2(400., 30.);

        for title in ["SOL", "SIGMA DRACONIS -> MINISTRY"] {
            let rect = shown(title, at, |ui| {
                spread(ui);
                ui.label("5 systems");
            });

            assert_eq!(rect.right_top(), at, "{title}");
        }
    }

    /// A system at `place`, otherwise as bare as [`system`]
    fn placed(address: i64, place: [f64; 3]) -> System {
        let mut system = system(address);
        system.position = place;
        system
    }

    /// Every distance the list puts on the lines of a route through `places`
    ///
    /// Read off what was painted rather than asked of the row, a row laying
    /// its own text out having no label to be asked what it says.
    ///
    /// The camera is a hundred light years off, so a distance measured from
    /// it could not be read as a jump.
    fn flown(places: &[[f64; 3]]) -> Vec<String> {
        let systems: Vec<System> = places
            .iter()
            .enumerate()
            .map(|(place, at)| placed(place as i64 + 1, *at))
            .collect();
        let route = Filter::Route {
            label: "A -> B".to_owned(),
            systems: (1..=places.len() as i64).collect(),
            range: "10".to_owned(),
        };

        crate::tests::words(|ui| {
            admitted(
                ui,
                &route,
                Some(&systems),
                Some(DVec3::new(100., 0., 0.)),
                &mut None,
                &mut None,
                &mut None,
            );
        })
        .into_iter()
        .filter(|said| said.ends_with(" Ly"))
        .collect()
    }

    /// A route says how far the jump that reaches each system is
    ///
    /// It is flown, so what is worth knowing about a system on one is whether
    /// the ship can get to it from the one before. How far it is from the
    /// camera answers a question nobody asked of a route.
    ///
    /// Two distances over three systems, the first being where the flying
    /// starts: no jump reaches it, so its line has nothing to say.
    #[test]
    fn a_route_says_how_far_each_jump_is() {
        let said = flown(&[[0., 0., 0.], [3., 4., 0.], [3., 4., 12.]]);

        assert_eq!(said, vec!["5.0 Ly", "12.0 Ly"], "{said:?}");
    }

    /// A set of systems says how far off each one is instead
    ///
    /// Nothing about a faction's holdings is a sequence, so there is no jump
    /// to measure and the distance the whole map is read in is what is left.
    #[test]
    fn a_set_of_systems_says_how_far_off_each_is() {
        let systems = [placed(1, [3., 4., 0.]), placed(2, [0., 0., 12.])];

        let said = crate::tests::words(|ui| {
            admitted(
                ui,
                &faction(7),
                Some(&systems),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
            );
        });

        assert!(said.contains(&"5.0 Ly".to_owned()), "{said:?}");
        assert!(said.contains(&"12.0 Ly".to_owned()), "{said:?}");
    }

    /// A route between `label`'s ends, plotted for a ship reaching `range`
    fn plotted_for(label: &str, range: &str) -> Filter {
        Filter::Route {
            label: label.to_owned(),
            systems: vec![1, 2],
            range: range.to_owned(),
        }
    }

    /// A route's panel says how far the ship it was plotted for reaches
    ///
    /// A panel is titled with what its filter is called and a route is called
    /// after its two ends, so the range is the one thing on screen telling two
    /// plots between the same pair apart.
    ///
    /// A range and not a jump. What the ship can cross in one is what the
    /// route was worked out against; what it actually crosses is on the lines
    /// below, and is usually less.
    #[test]
    fn a_route_panel_says_the_range_it_was_plotted_for() {
        assert_eq!(
            summary(&plotted_for("SOL -> BARNARD", "10"), 12),
            "12 systems, 10 Ly range"
        );
    }

    /// Two plots between the same ends are told apart by it
    ///
    /// Which is the whole of why it is said. Both panels are titled the same,
    /// both list systems between the same two, and what the user asked for is
    /// the difference between them.
    #[test]
    fn two_routes_between_the_same_ends_read_apart() {
        let near = summary(&plotted_for("SOL -> BARNARD", "10"), 12);
        let far = summary(&plotted_for("SOL -> BARNARD", "20"), 7);

        assert_ne!(near, far);
        assert!(near.contains("10 Ly"), "{near}");
        assert!(far.contains("20 Ly"), "{far}");
    }

    /// A filter that was never plotted says how many and no more
    ///
    /// A faction and a hand-picked set were never asked a range, and a panel
    /// that answered one for them would be answering for the user.
    #[test]
    fn a_filter_that_was_not_plotted_says_only_how_many() {
        assert_eq!(summary(&faction(7), 12), "12 systems");
        assert_eq!(
            summary(
                &Filter::Systems {
                    label: "3 systems".to_owned(),
                    systems: vec![1, 2, 3],
                },
                3
            ),
            "3 systems"
        );
    }

    /// And the panel draws whatever the summary came to
    ///
    /// Read off what was painted, the line being a label the panel lays out
    /// from what `summary` answered.
    #[test]
    fn the_panel_draws_the_summary() {
        let systems = [placed(1, [0., 0., 0.]), placed(2, [3., 4., 0.])];

        let said = crate::tests::words(|ui| {
            admitted(
                ui,
                &plotted_for("SOL -> BARNARD", "10"),
                Some(&systems),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
            );
        });

        assert!(
            said.contains(&"2 systems, 10 Ly range".to_owned()),
            "{said:?}"
        );
    }

    /// A line carries the id its filter would be built from
    ///
    /// Clicking a faction asks for it, and what a filter tests against is the
    /// id, so the name alone would leave the line unable to say which faction
    /// it named.
    #[test]
    fn a_listed_faction_carries_its_id() {
        let names = known(&[(7, "Alliance of Sol")]);

        assert_eq!(listed(&[7], &names), vec![(7, Some("Alliance of Sol"))]);
    }

    /// A faction whose name has not arrived keeps its line, at the end
    ///
    /// Naming them is one query behind the panel opening, and a list that
    /// grew a line a frame later would jump under the reader. Held at the
    /// end rather than sorted in, since the placeholder is punctuation and
    /// would otherwise sit above every name and then jump the length of the
    /// list as soon as its own arrived.
    ///
    /// It answers to nothing while it waits. A faction is asked for by name
    /// as well as by id, and a filter row naming one as punctuation would say
    /// nothing about what had gone dim.
    #[test]
    fn a_faction_not_yet_named_takes_the_last_line() {
        let names = known(&[(1, "Alliance of Sol")]);
        let wanted = vec![(1, Some("Alliance of Sol")), (2, None)];

        assert_eq!(listed(&[1, 2], &names), wanted);
        assert_eq!(listed(&[2, 1], &names), wanted);
    }

    /// Each panel takes the place after the last one opened
    #[test]
    fn panels_take_one_place_after_another() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(2));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0, 1]);
    }

    /// Shut a panel and its place goes to the next one opened
    ///
    /// Reading one system and then another is the common way to use these,
    /// and a place kept for a panel that is gone leaves the second one
    /// opening below a gap.
    #[test]
    fn a_shut_panel_hands_its_place_on() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        shut(&mut panels, system(1));
        panels.open_system(system(2));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0]);
    }

    /// The place handed on is the first free one, not the last one shut
    #[test]
    fn a_panel_fills_the_first_gap() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(2));
        panels.open_system(system(3));
        shut(&mut panels, system(2));
        panels.open_system(system(4));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0, 2, 1]);
    }

    /// No two panels stand in one place
    ///
    /// Whatever has been opened and shut in between. Two panels sharing a
    /// place is two windows on top of each other, with the lower one only
    /// findable by dragging the upper one off it.
    #[test]
    fn panels_do_not_share_a_place() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(2));
        panels.open_system(system(3));
        shut(&mut panels, system(1));
        shut(&mut panels, system(3));
        panels.open_system(system(4));
        // The one shut first, opened again alongside what took its place.
        panels.open_system(system(1));

        let mut slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        let held = slots.len();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), held);
    }

    /// A panel is put where the tiling says on the frame it opens
    ///
    /// Egui remembers where a window was for the whole session, shut windows
    /// included. A panel opened a second time takes whatever place is free
    /// then, and asking for that place as a default would leave egui
    /// answering with the place it had before, which something else may be
    /// standing in.
    #[test]
    fn a_panel_is_placed_when_it_opens() {
        let mut panels = Panels::default();
        panels.open_system(system(1));

        assert!(!panels.open[0].placed);
    }

    /// Shut the panel about `system`, as clicking its cross does
    fn shut(panels: &mut Panels, system: System) {
        let shut = Subject::System(system).id();
        panels.open.retain(|panel| panel.subject.id() != shut);
    }

    /// The first panel opens in the corner
    #[test]
    fn the_first_panel_takes_the_corner() {
        assert_eq!(tile(0, 3, 2), (0, 0));
    }

    /// The next opens below it rather than on it
    #[test]
    fn panels_tile_down_the_edge() {
        assert_eq!(tile(1, 3, 2), (1, 0));
        assert_eq!(tile(2, 3, 2), (2, 0));
    }

    /// A full column starts a fresh one to its left
    #[test]
    fn a_full_column_moves_across() {
        assert_eq!(tile(3, 3, 2), (0, 1));
        assert_eq!(tile(5, 3, 2), (2, 1));
    }

    /// A full viewport starts over in the corner
    ///
    /// The one place two panels are left on top of each other. There is
    /// nowhere else for the next one to go.
    #[test]
    fn a_full_viewport_starts_over() {
        assert_eq!(tile(6, 3, 2), (0, 0));
    }

    /// A viewport with room for nothing still answers
    ///
    /// The tiling is measured against a window the user can drag as small as
    /// they like, and dividing by what is left is how that would come back
    /// as a crash rather than as a cramped panel.
    #[test]
    fn no_room_is_still_a_place() {
        assert_eq!(tile(0, 0, 0), (0, 0));
        assert_eq!(tile(4, 0, 0), (0, 0));
    }

    /// A system already being read about is not opened twice
    #[test]
    fn one_system_gets_one_panel() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(1));

        assert_eq!(panels.open.len(), 1);
    }

    /// A faction filter, by id, called after it
    fn faction(id: i32) -> Filter {
        Filter::Faction { id, name: format!("Faction {id}") }
    }

    /// A filter already being read about is not opened twice
    #[test]
    fn one_filter_gets_one_panel() {
        let mut panels = Panels::default();
        panels.open_filter(faction(7));
        panels.open_filter(faction(7));

        assert_eq!(panels.open.len(), 1);
    }

    /// Two filters get a panel each
    #[test]
    fn each_filter_gets_its_own_panel() {
        let mut panels = Panels::default();
        panels.open_filter(faction(7));
        panels.open_filter(faction(9));

        assert_eq!(panels.open.len(), 2);
    }

    /// A filter and a system are never the same panel
    ///
    /// They share the tiling and the window, so nothing but the identity
    /// keeps a filter's panel from being taken for the panel of a system
    /// that happened to open at the same time.
    #[test]
    fn a_filter_and_a_system_are_different_panels() {
        let mut panels = Panels::default();
        panels.open_system(system(7));
        panels.open_filter(faction(7));

        assert_eq!(panels.open.len(), 2);
    }

    /// What the database does not say is said to be unknown
    ///
    /// Most of what is recorded about a system is optional, and a blank row
    /// reads as the panel having failed rather than as an answer.
    #[test]
    fn what_is_not_recorded_says_so() {
        assert_eq!(named(&Some(Allegiance::Empire)), "Empire");
        assert_eq!(named::<Allegiance>(&None), "Unknown");
    }
}
