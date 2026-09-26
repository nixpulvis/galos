//! The window a panel stands in: how wide, how tall, where it is tiled and
//! what it is titled

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Context, Ui};

/// How wide a panel stands
///
/// Wide enough for a position and for the longest of the names a field is
/// answered with, so that the two columns do not shift from one system to
/// the next.
///
/// A panel comes out at this taken up to a whole character, the title bar
/// being lettered and filled out to its end.
pub(super) const WIDTH: f32 = 230.;

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
    crate::ui::text::covering(ctx, egui::TextStyle::Body, WIDTH - TITLE_MARKS)
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
    let said = crate::ui::text::shortened(title, room);
    let spare = room.saturating_sub(said.chars().count());

    egui::RichText::new(format!("{said}{}", " ".repeat(spare)))
        .text_style(egui::TextStyle::Body)
}

/// The least room a panel is ever offered for what it holds, in pixels
///
/// A viewport too short to hold a panel is a viewport too short to hold
/// anything; what it gets is a couple of rows and a scroll bar rather than a
/// window egui refuses to draw.
const LEAST: f32 = 64.;

/// The box a panel is kept inside: the viewport, a margin off every edge
///
/// Where it may stand, and how tall: the room under it is measured from here.
fn kept_inside(ctx: &Context) -> egui::Rect {
    ctx.content_rect().shrink(crate::ui::MARGIN)
}

/// The room a panel standing at `top` has for what it holds, in pixels
///
/// Down to the bottom of the box it is kept inside, less `frame` — how much of
/// a panel is the window round it, which is [`Panels::frame`](crate::ui::panels::Panels::frame) and is measured
/// rather than worked out.
///
/// Measured from where the panel actually stands, which is where it stood last
/// frame: a panel the user has dragged halfway down the screen is capped by
/// the room where it is rather than by the room the tiling would have given it.
pub(super) fn room_under(
    ctx: &Context,
    id: egui::Id,
    at: egui::Pos2,
    frame: f32,
) -> f32 {
    let stood =
        ctx.memory(|memory| memory.area_rect(id).map(|rect| rect.top()));
    (kept_inside(ctx).bottom() - stood.unwrap_or(at.y) - frame).max(LEAST)
}

/// The window a panel stands in, put where the tiling says
///
/// `at` is where its right hand top corner goes, that being the corner the
/// tiling works from: panels stand against the right edge of the viewport, and
/// a window is at least as wide as its title bar needs, so where its left edge
/// falls is not known until it has been drawn.
///
/// `placed` is [`Panel::placed`](crate::ui::panels::Panel::placed): a panel is put where the tiling says on the
/// frame it opens, and asked for `at` as a default after that, so that one
/// dragged somewhere stays where it was dragged.
///
/// `room` is what it has for what it holds, and is both the height it opens
/// filling where it has more to show than that and the height a dragged one is
/// held to. Egui does its own clamping against the box a window is constrained
/// to, and does not take the window's own margins off when it does, so the
/// room is the figure to trust.
///
/// The title is cut to the room there is for it, both ends of a route kept.
/// A panel as wide as its name is a panel the tiling cannot place and the
/// user cannot read two of side by side.
pub(super) fn framed<'open>(
    ctx: &Context,
    title: &str,
    id: egui::Id,
    at: egui::Pos2,
    room: f32,
    placed: bool,
    showing: &'open mut bool,
) -> egui::Window<'open> {
    let window = egui::Window::new(titled(ctx, title))
        .id(id)
        .open(showing)
        // The height alone. A panel is as wide as its two columns and its
        // title bar need and no wider, so there is nothing to drag there; how
        // much of a long list to show is the user's business.
        .resizable([false, true])
        .pivot(egui::Align2::RIGHT_TOP)
        // Over the chrome, which is what a window is: the pane and the bar
        // stand where the map put them and a panel stands where the user
        // dragged it, so where the two meet the window is the one on top.
        // Said here rather than left to a window's own `Order::Middle`, which
        // is where the chrome sits: same-order areas are stacked by which was
        // last interacted with, and a panel would slide under the pane the
        // moment the pane was touched.
        .order(egui::Order::Foreground)
        // The width alone. Left unsaid it is `Style::default_area_size`, 600,
        // which will not fit where a panel is asked to be placed, so egui
        // slides the window somewhere it does and remembers it there.
        .default_width(WIDTH)
        // The room, both ways. Preferred, so a panel with more to show than
        // fits opens filling it rather than at egui's own default of four
        // hundred pixels with the screen empty under it; and at most, so a
        // height the user has dragged is held to it and a viewport shrinking
        // under a tall panel brings it back inside.
        .default_height(room)
        .max_height(room)
        // On the screen, wherever it was dragged to.
        .constrain_to(kept_inside(ctx));

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

/// A panel's contents: across the whole of the window, and scrolled inside the
/// `room` there is for them — answering how tall they came out
///
/// A panel is as tall as what it holds — four rows of a body's orbit, or four
/// hundred systems of a faction's holdings — and nothing about a window bounds
/// that, so a long one ran off the bottom of the viewport with the rest of it
/// out of reach. Scrolled, the panel stops at the room there is and the bar
/// carries the rest.
///
/// The room rather than `ui.available_height()`, which is what the window has
/// already decided to be: read from there a long panel can never ask for more
/// than it was given last frame, so it never grows into the room it has.
///
/// No height is imposed: [`crate::ui::list::scrolling`] grows to what it is given
/// and stops at what is in it, so a panel of four rows is four rows tall.
/// Which is what the tiling wants — it steps the next panel by the tallest
/// drawn — and what a fixed height would take away.
///
/// What comes back is the height the contents came out at, which is the other
/// half of measuring [`Panels::frame`](crate::ui::panels::Panels::frame).
pub(super) fn inside(
    ui: &mut Ui,
    id: egui::Id,
    room: f32,
    contents: impl FnOnce(&mut Ui),
) -> f32 {
    spread(ui);
    crate::ui::list::scrolling(ui, room, id, contents);
    ui.min_rect().height()
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
pub(super) fn tile(slot: usize, down: usize, across: usize) -> (usize, usize) {
    let down = down.max(1);
    let slot = slot % (down * across.max(1));
    (slot % down, slot / down)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MARGIN;

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
        let ctx = crate::testing::context();
        let mut rect = egui::Rect::ZERO;

        // A screen to stand on. A panel is kept inside the viewport now, so
        // one opened on a context with no screen at all is pushed to wherever
        // egui's default rect leaves room for it.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600., 600.),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let mut showing = true;
            let id = egui::Id::new("test-panel");
            let panel = framed(
                ui.ctx(),
                title,
                id,
                at,
                room_under(ui.ctx(), id, at, 0.),
                false,
                &mut showing,
            );
            if let Some(panel) = panel.show(ui.ctx(), &mut contents) {
                rect = panel.response.rect;
            }
        });

        rect
    }

    /// How tall a panel of `rows` lines comes out, on each screen in turn
    ///
    /// Drawn where the tiling opens one, against the top right of the screen,
    /// and through [`inside`] as a panel's contents always are — including the
    /// measuring of the window round them, which is what the room is worked
    /// out from. A few frames per screen, since that measurement is a frame
    /// old, and one context throughout, so a screen that shrinks under a panel
    /// already standing is a shrink rather than a fresh panel.
    fn stands(screens: &[f32], rows: usize) -> f32 {
        let ctx = crate::testing::context();
        let id = egui::Id::new("tall-panel");
        let at = egui::pos2(600. - MARGIN, MARGIN);
        let mut rect = egui::Rect::ZERO;
        let mut framing = 0.;
        for high in screens {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., *high),
                )),
                ..Default::default()
            };
            for _ in 0..4 {
                let _ = ctx.run_ui(input.clone(), |ui| {
                    let mut showing = true;
                    let room = room_under(ui.ctx(), id, at, framing);
                    let panel = framed(
                        ui.ctx(),
                        "PANEL",
                        id,
                        at,
                        room,
                        false,
                        &mut showing,
                    );
                    let mut held = 0.;
                    let shown = panel.show(ui.ctx(), |ui| {
                        held = inside(ui, id, room, |ui| {
                            for row in 0..rows {
                                ui.label(format!("row {row}"));
                            }
                        });
                    });
                    if let Some(shown) = shown {
                        rect = shown.response.rect;
                        framing = rect.height() - held;
                    }
                });
            }
        }

        rect.height()
    }

    /// A panel is as tall as what it holds, up to the room there is
    ///
    /// A panel holds anything from four rows of an orbit to four hundred
    /// systems of a faction's holdings, and it ran off the bottom of the
    /// viewport with the rest of itself out of reach: the contents are
    /// scrolled now, so a long one stops at the room.
    ///
    /// It fills that room rather than egui's own default of four hundred
    /// pixels for a window with a scroll area in it, which left a long panel
    /// short with the screen empty under it. And no height is imposed to do
    /// either: the tiling steps the next panel by the tallest drawn, so a
    /// short panel held to the room would leave a column with one panel in it
    /// and a screen of nothing under it.
    #[test]
    fn a_panel_is_as_tall_as_what_it_holds_up_to_the_room() {
        let short = stands(&[600.], 4);
        let long = stands(&[600.], 400);

        assert!(short < 200., "four rows stood {short} tall");
        assert!(
            long <= 600.,
            "four hundred rows stood {long} tall, past a 600 screen"
        );
        assert!(
            long > 500.,
            "four hundred rows stood {long} tall, well short of the room"
        );
    }

    /// And comes back inside a viewport that shrinks under it
    ///
    /// The reported trouble: a panel standing the height of the screen and
    /// then the screen made shorter kept the height it had, so the end of it —
    /// the buttons under a system's factions — was off the bottom with nothing
    /// to reach it by. The room is measured afresh every frame, from where the
    /// panel actually stands, and caps the height as well as suggesting it.
    #[test]
    fn a_panel_comes_back_inside_a_shrinking_viewport() {
        let shrunk = stands(&[600., 300.], 400);

        assert!(
            shrunk <= 300.,
            "the panel stood {shrunk} tall on a screen of 300"
        );
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
            egui::Frame::window(&crate::testing::context().global_style())
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
        let ctx = crate::testing::context();
        // Inside a pass, egui having no fonts to measure with before one.
        let mut one = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            one =
                crate::ui::text::one_character(ui.ctx(), egui::TextStyle::Body);
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
        let ctx = crate::testing::context();
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
                    room_under(ui.ctx(), id, egui::Pos2::ZERO, 0.),
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
        let ctx = crate::testing::context();
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
}
