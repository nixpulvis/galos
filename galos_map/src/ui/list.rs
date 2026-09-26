//! A list of systems, a line to each, and what a press on a line or a row meant
//!
//! Shared by the bar and the panels, so that a line in either reads, lights
//! and answers a click the same way.

use crate::ui::INFO;
use crate::ui::text::width;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Response, Ui};

/// What the map can be asked to do with one system
///
/// The same ones the map itself answers, wherever the line was drawn: one
/// click says which system is meant, a modifier held with it says as well as
/// the rest, and a second click says to go there. Shared by every list of
/// systems the map draws, so that reaching one through a search and reaching
/// one through a filter are the same gesture rather than two to be learned.
pub(crate) enum SystemAction {
    /// Pick the system out, as clicking a star does
    ///
    /// `gathering` holds the modifier rather than a variant of its own,
    /// because holding it does not ask for something else. It is the one
    /// gesture either way, saying which system is meant; all the modifier
    /// says is whether the rest are meant along with it.
    Select { gathering: bool },
    /// Send the camera to it, as double clicking a star does
    Travel,
    /// Say what is known about it, and leave the selection alone
    Describe,
}

/// Whether a key is down asking for as well as rather than instead
///
/// The same gesture the sky answers, so that a line in the list and the star
/// it names are picked out the same way. Command covers control where the
/// user came from Windows or Linux and the cloverleaf where they came from a
/// Mac, and shift stands beside them as the one no platform reads as asking
/// for something else.
///
/// The same three [`crate::map::galaxy::spawn`] asks the keyboard for directly.
pub(crate) fn gathering(ui: &Ui) -> bool {
    ui.input(|input| {
        let keys = input.modifiers;
        keys.command || keys.ctrl || keys.shift
    })
}

/// What one press on a filter's row meant
///
/// Six things can be pressed in the space of a row, and a press lands on
/// exactly one of them. Kept apart from the drawing because the order is the
/// whole of it: [`asked_of_row`] is where that order is written down and the
/// only place it can be got wrong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RowGesture {
    /// Take the filter away for good
    LetGo,
    /// Open the panel describing it
    Describe,
    /// Ask the route it names over, from nothing
    Replot,
    /// Turn it off, or back on
    Toggle,
    /// Send the camera to see the whole of what it admits
    Frame,
    /// Say it is the one being worked with
    Select,
}

/// Which of them a press on a row was
///
/// The marks first, in the order they are drawn — close, info, again — then
/// the switch, then the double. Egui answers the first click of a pair as a
/// click and the second as a double, so a row double
/// clicked has already been asked about as a click by the time this is
/// reached: the double has to beat the click or framing a filter would pick
/// it out on the way, which is the same order the selection rows are read in.
///
/// The switch beats the row for the plainer reason that it stands inside it.
/// A press on the dot is a press on the row as well, and it means the dot.
///
/// Read here by everything a press can land on that stands for one filter or
/// one system: the bar's rows, the sections over them, and the legs a trip's
/// panel lists. A row that offers fewer of the six says so by passing
/// `false`, rather than keeping an order of its own.
pub(crate) fn asked_of_row(
    close: bool,
    info: bool,
    again: bool,
    switch: bool,
    double: bool,
    click: bool,
) -> Option<RowGesture> {
    if close {
        Some(RowGesture::LetGo)
    } else if info {
        Some(RowGesture::Describe)
    } else if again {
        Some(RowGesture::Replot)
    } else if switch {
        Some(RowGesture::Toggle)
    } else if double {
        Some(RowGesture::Frame)
    } else if click {
        Some(RowGesture::Select)
    } else {
        None
    }
}

/// Whether a click on `row` is a click, and not the first half of a double
///
/// egui raises `clicked()` on the first release of a double click, a frame
/// before anything reports the double at all — measured: frame one
/// `clicked` alone, frame two `clicked` and `double_clicked` together. So
/// [`asked_of_row`]'s priority, which arbitrates the flags of one pass and
/// gets frame two right, has already been handed frame one and acted on it.
///
/// Which matters wherever [`RowGesture::Select`] does something a double
/// click is not supposed to do. It replaces what is picked out, so
/// double clicking a route's row to fly to it first wiped whatever the user
/// was holding and put the route's own ends there instead — and the double
/// then framed it, over a selection it had no business changing.
///
/// So the click is held for the window a double may still arrive in, and
/// answered only once it has passed. A double inside it takes the pending
/// click away with it. The wait is egui's own `max_double_click_delay`,
/// three tenths of a second, and it is the wait a double click already costs
/// the gesture it is not.
///
/// What comes back is the modifiers of the *press*, since that is what the
/// gesture meant and the hand is off the key by the time the window has
/// passed. See [`gathering_with`].
///
/// Kept in egui's own per-id store rather than in a resource: it is one
/// press per row, it belongs to the row, and it goes when the row does.
/// A row must be drawn to be answered — a pending click on a row that stops
/// being listed is never returned, which is the same as the row's press
/// never having been read.
pub(crate) fn settled_click(
    ui: &Ui,
    row: egui::Id,
    click: bool,
    double: bool,
) -> Option<egui::Modifiers> {
    let pending = row.with("click awaiting a double");
    let now = ui.input(|input| input.time);

    if double {
        ui.data_mut(|data| data.remove::<(f64, egui::Modifiers)>(pending));
        return None;
    }
    if click {
        let keys = ui.input(|input| input.modifiers);
        ui.data_mut(|data| data.insert_temp(pending, (now, keys)));
        return None;
    }
    let held: Option<(f64, egui::Modifiers)> =
        ui.data(|data| data.get_temp(pending));
    let waited = ui.ctx().options(|it| it.input_options.max_double_click_delay);
    match held {
        Some((since, keys)) if now - since >= waited => {
            ui.data_mut(|data| data.remove::<(f64, egui::Modifiers)>(pending));
            Some(keys)
        }
        _ => None,
    }
}

/// Whether a press meant "and these as well", read off the press itself
///
/// [`gathering`] asks the frame it is called in, which is right for a gesture
/// acted on where it lands. A click held to see whether a double follows is
/// not: by the time it is answered the modifier has been let go of, and the
/// press would read as a bare one. So the keys travel with the pending click
/// and are asked here.
pub(crate) fn gathering_with(keys: egui::Modifiers) -> bool {
    keys.command || keys.ctrl || keys.shift
}

/// How far a line in a list holds its text off its own edge
pub(crate) const LINE_PADDING: f32 = 3.;

/// One full width line of a list, and the pointer's answer to it
///
/// The whole line answers rather than the letters on it, so that a short name
/// is as easy to hit as a long one and a list reads as a column of controls
/// rather than as text that happens to be clickable. Laid out and painted for
/// the reason the rows in the bar are: a label is a widget in its own right,
/// and one inside a row that also answers leaves the two bidding for the
/// pointer.
///
/// `reserved` is room kept clear at the right hand end, which the caller
/// paints into itself. The rect handed back is the whole line, so it knows
/// where that room ended up.
///
/// A line that is not a `control` is laid out the same and answers to nothing:
/// it neither lights under the pointer nor takes the hand cursor, so a list
/// holding one keeps its shape without offering something that cannot be had.
pub(crate) fn line(
    ui: &mut Ui,
    text: egui::RichText,
    reserved: f32,
    control: bool,
) -> (egui::Rect, Response) {
    let room = ui.available_width() - LINE_PADDING * 2. - reserved;
    let text = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        room.max(0.),
        egui::TextStyle::Body,
    );

    let height = text.size().y;
    let (rect, answer) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height + LINE_PADDING * 2.),
        if control { egui::Sense::click() } else { egui::Sense::empty() },
    );

    if control && (answer.hovered() || answer.has_focus()) {
        ui.painter().rect_filled(
            rect,
            ui.visuals().widgets.hovered.corner_radius,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    ui.painter().galley(
        egui::pos2(rect.left() + LINE_PADDING, rect.center().y - height / 2.),
        text,
        // A real color, since a line is laid out from whatever the caller
        // hands over and that is usually plain text. Plain text carries no
        // color of its own, so it comes out of layout as a placeholder for
        // this to answer, and a placeholder answered by a placeholder reaches
        // the tessellator, which panics rather than guess.
        ui.visuals().text_color(),
    );

    if control {
        (rect, answer.on_hover_cursor(egui::CursorIcon::PointingHand))
    } else {
        (rect, answer)
    }
}

/// Whether a list's lines carry their reading beside the name or under it
///
/// **Settled for the list, not for the line.** A name is the one thing a row
/// cannot do without — `CO…` is not a system — so where a name and its
/// reading will not both fit, the name takes the width and the reading goes
/// on a line of its own beneath. Deciding that per line meant a list in
/// which some stops read one way and some the other, the column of readings
/// breaking wherever a long name happened to fall, and it was reported as
/// exactly that. So the widest line in the list decides for all of them: a
/// list is one thing and reads one way.
///
/// The cost is height, which a list of a hundred stops spends carefully, and
/// it is spent on the whole list or none of it. Short lists — a search's
/// results, a system's neighbours — go on reading in one line each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rows {
    /// The reading at the right hand end, so the column reads down
    Beside,
    /// The reading indented under the name, the name having the width
    Under,
}

impl Rows {
    /// How a list reads, given the room it gets and every name and reading
    /// it is about to draw
    ///
    /// `room` is the width one of its lines will be drawn in, which is the
    /// caller's to say rather than this to assume: a trip's stops are
    /// indented under their leg's name and so get an indent less than the
    /// panel they stand in. Asked before any of the lines are drawn, since
    /// the first line has to know what the last one needs.
    pub(crate) fn of<'a>(
        ui: &Ui,
        room: f32,
        lines: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    ) -> Self {
        let gap = ui.spacing().item_spacing.x;
        // What is left for the name and the reading once the line's own
        // padding and the mark that opens a panel have taken theirs.
        let room = room - LINE_PADDING * 2. - gap - width(ui, INFO);

        for (name, trailing) in lines {
            let Some(trailing) = trailing else { continue };
            if width(ui, name) + gap + width(ui, trailing) > room {
                return Self::Under;
            }
        }
        Self::Beside
    }
}

/// One system's line in a list, and what a click on it asked for
///
/// Every list of systems the map draws is this line: the ones a search found
/// and the ones a filter admits, so far. They are the same thing in two
/// places, and a change to how a system is picked out of a list belongs in one
/// of them rather than in each.
///
/// `trailing` is what stands at the right hand end, before the mark. Usually
/// how far off the system is, and in the same slot whatever it says, so the
/// column reads down.
///
/// `rows` is where that reading goes, and is the list's to decide rather
/// than the line's — see [`Rows`].
///
/// `salt` keys the mark apart from the marks on the lines around it. The
/// caller chooses it, knowing what its own list does between one pass and the
/// next.
pub(crate) fn system_line(
    ui: &mut Ui,
    name: &str,
    trailing: Option<String>,
    rows: Rows,
    salt: impl std::hash::Hash,
) -> Option<SystemAction> {
    let gap = ui.spacing().item_spacing.x;
    let trailing = trailing.map(|text| {
        egui::WidgetText::from(egui::RichText::new(text).weak()).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    });

    // Every list the map draws is of systems it can place, so every line
    // carries the mark that opens a panel on one.
    let mark = {
        // Laid out in nothing, so the color can be chosen once the pointer
        // has been asked about, which cannot happen until the line has been
        // placed.
        egui::WidgetText::from(
            egui::RichText::new(INFO).color(egui::Color32::PLACEHOLDER),
        )
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    };

    // The reading's own room, where it is beside the name: under it the
    // name has the whole line.
    let beside = match rows {
        Rows::Beside => {
            trailing.as_ref().map_or(0., |text| text.size().x + gap)
        }
        Rows::Under => 0.,
    };
    let reserved = mark.size().x + gap + beside;
    let (rect, answer) = line(ui, egui::RichText::new(name), reserved, true);
    let middle = rect.center().y;

    // Asked about after the line, so that it is the one answering where the
    // two overlap. Under it the line would have to work out what it was not
    // being clicked on.
    let describing = {
        let at = egui::Rect::from_min_max(
            egui::pos2(rect.right() - LINE_PADDING - mark.size().x, rect.top()),
            egui::pos2(rect.right() - LINE_PADDING, rect.bottom()),
        );
        let answer = ui.interact(
            at,
            ui.id().with(("describe", salt)),
            egui::Sense::click(),
        );
        // Brightened alone, as the marks in the bar are. The line beneath it
        // already lights up under the pointer, and a rectangle inside that
        // one reads as a button dropped into the line.
        let lit = answer.hovered() || answer.has_focus();
        let height = mark.size().y;
        ui.painter().galley(
            egui::pos2(at.left(), middle - height / 2.),
            mark,
            if lit {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            },
        );
        (at, answer.on_hover_cursor(egui::CursorIcon::PointingHand))
    };

    // Between the name and the mark, right against the mark, so the
    // distances line up down the list rather than following the names — or,
    // where the two would not fit on one line, under the name in its own
    // row. Indented there, so a run of them reads as belonging to the names
    // above rather than as a list of its own.
    if let Some(text) = trailing {
        match rows {
            Rows::Beside => {
                let size = text.size();
                let right = describing.0.left() - gap;
                ui.painter().galley(
                    egui::pos2(right - size.x, middle - size.y / 2.),
                    text,
                    egui::Color32::PLACEHOLDER,
                );
            }
            Rows::Under => {
                ui.horizontal(|ui| {
                    ui.add_space(LINE_PADDING + gap);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text.text()).weak(),
                        )
                        .selectable(false),
                    );
                });
            }
        }
    }

    // The mark first, then the double. Egui answers the first click of a pair
    // as a click and the second as a double, so a line double clicked has
    // already been picked out by the time this is asked, which is what the
    // first click of the pair was for.
    if describing.1.clicked() {
        Some(SystemAction::Describe)
    } else if answer.double_clicked() {
        Some(SystemAction::Travel)
    } else if answer.clicked() {
        Some(SystemAction::Select { gathering: gathering(ui) })
    } else {
        None
    }
}

/// A scrolling list whose bar stands beside its contents rather than over them
///
/// Egui floats a scroll bar over the top right corner of what it is
/// scrolling. That reads well over a paragraph and not at all over a list
/// whose lines carry a control at that end: the bar and the mark are drawn on
/// the same few pixels, and whichever the pointer lands on is a coin toss.
///
/// A bar that is laid out is taken out of the room its contents are given, so
/// a line ends where the bar begins and the two never meet. It costs the
/// width of the bar, and only while there is more to scroll to: egui shows
/// one when it is needed and takes no room when it is not.
///
/// Grows to `height` and no further, and no taller than what is in it, so a
/// list of three lines is three lines rather than one with room going spare.
///
/// The room is asked for rather than read off the `Ui`. A scroll area holds
/// itself to whatever height it is offered, and inside an [`egui::Area`] what
/// is on offer is the height the area came out at *last* frame: egui lays an
/// area's contents out in the rectangle the last pass left behind. So a list
/// that came back under a bar which had been shut was offered the shut bar's
/// height — three lines of the five — and it stayed three, the area coming
/// out that tall again on the strength of it and offering no more the frame
/// after. Asked for, the room is the room whatever the area last was; the
/// rectangle is allocated at the height wanted and the space actually taken
/// is what the caller is charged, so a short list is still short.
pub(crate) fn scrolling<R>(
    ui: &mut Ui,
    height: f32,
    salt: impl std::hash::Hash,
    contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    // In a ui of its own, since a style set on a `Ui` is set on the rest of
    // that `Ui`, and this is asked for by the list rather than by whatever
    // follows it.
    ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
        ui.spacing_mut().scroll.floating = false;
        egui::ScrollArea::vertical()
            // Named by the caller, since the bar holds several of these at
            // once and a scroll area left to work its own id out from where it
            // sits gets the same one as the next: egui says so out loud, in
            // red, over the map. Each list is its own place to have scrolled
            // to anyway, and where one has been scrolled to says nothing about
            // the others.
            .id_salt(salt)
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, contents)
            .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{context, painted, words};

    /// A long name with a long reading, and a short one with a short one
    const LONG: (&str, &str) =
        ("SWOIWNS TW-F C26-1204", "neutron star, 279.6 Ly");

    const SHORT: (&str, &str) = ("SOL", "61.5 Ly");

    /// How a list of these lines reads in a panel `room` wide
    fn reading(room: f32, lines: &[(&str, &str)]) -> Rows {
        let ctx = context();
        let mut rows = Rows::Beside;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            rows = Rows::of(
                ui,
                room,
                lines.iter().map(|(name, after)| (*name, Some(*after))),
            );
        });
        rows
    }

    /// One line's height, drawn the way `rows` says
    ///
    /// The height is the observable: a truncated galley still reports its
    /// whole text, so what says the reading moved to its own line is that
    /// the row came out two lines tall.
    fn tall(room: f32, rows: Rows) -> f32 {
        let ctx = context();
        let mut tall = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_width(room);
            let top = ui.cursor().top();
            system_line(ui, LONG.0, Some(LONG.1.to_owned()), rows, "measured");
            tall = ui.cursor().top() - top;
        });
        tall
    }

    /// A name is not crushed to make room for what stands after it
    ///
    /// The reported trouble: a route's stops carry a star class and a jump
    /// distance, and against the panel's width that left `CO…` and `SW…`
    /// where a system's name should be. A name is the one thing a row
    /// cannot do without, so where the two will not fit the reading goes
    /// under it and the name takes the width — two lines of a scrolling
    /// list, which the panel has room for, against a name that has lost its
    /// letters.
    #[test]
    fn a_narrow_list_puts_the_reading_under_the_name() {
        assert_eq!(reading(110., &[LONG]), Rows::Under);
        assert_eq!(reading(600., &[LONG]), Rows::Beside);

        let under = tall(110., Rows::Under);
        let beside = tall(110., Rows::Beside);
        assert!(
            under > beside * 1.5,
            "a spilled row came out {under} tall against {beside}, so the \
             reading did not move under the name",
        );
    }

    /// And every line of that list reads the same way
    ///
    /// Reported after the first go at it, which decided line by line: one
    /// long name among short ones put its own reading underneath and left
    /// the rest beside their names, so the column of readings broke
    /// wherever a long name happened to fall. A list is one thing and reads
    /// one way, so the line that needs the room decides for all of them.
    #[test]
    fn one_long_name_settles_the_whole_list() {
        assert_eq!(reading(230., &[SHORT, SHORT]), Rows::Beside);
        assert_eq!(reading(230., &[SHORT, LONG, SHORT]), Rows::Under);
    }

    /// A line with nothing after its name settles nothing
    ///
    /// A list where some systems have a reading and some have none is not a
    /// reason to spend the height on any of them: what cannot fit is a name
    /// and a reading together, and a name on its own has the width already.
    #[test]
    fn a_line_with_no_reading_does_not_settle_it() {
        let ctx = context();
        let mut rows = Rows::Under;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            rows = Rows::of(
                ui,
                110.,
                [(LONG.0, None), (SHORT.0, Some(SHORT.1))].into_iter(),
            );
        });

        assert_eq!(rows, Rows::Beside);
    }

    /// Both are still said, either way round
    #[test]
    fn a_row_says_the_name_and_the_reading() {
        for rows in [Rows::Beside, Rows::Under] {
            let said = words(|ui| {
                ui.set_max_width(400.);
                system_line(
                    ui,
                    "SOL",
                    Some("neutron star, 61.5 Ly".to_owned()),
                    rows,
                    "both",
                );
            });

            assert!(said.iter().any(|line| line == "SOL"), "{said:?}");
            assert!(
                said.iter().any(|line| line.contains("neutron star")),
                "{said:?}"
            );
        }
    }

    /// What a press on a row means depends on where in it it landed
    ///
    /// Six things share the space of a row and a press lands on one of them.
    /// The order is the whole of the rule, and two pairs are why it is
    /// written down rather than left to a chain of ifs nobody re-reads.
    #[test]
    fn a_press_on_a_row_means_the_one_thing_it_landed_on() {
        assert_eq!(
            asked_of_row(false, false, false, false, false, false),
            None
        );

        // The dot stands inside the row, so a press on it is a press on the
        // row as well -- and it means the dot. Otherwise turning a filter off
        // would pick it out on the way.
        assert_eq!(
            asked_of_row(false, false, false, true, false, true),
            Some(RowGesture::Toggle)
        );
        // Egui answers the first click of a pair as a click, so a double
        // arrives with a click beside it. The double has to win, or framing a
        // filter would pick it out first.
        assert_eq!(
            asked_of_row(false, false, false, false, true, true),
            Some(RowGesture::Frame)
        );
        // The marks at the end beat all of it: they are what was pressed.
        assert_eq!(
            asked_of_row(true, false, false, true, true, true),
            Some(RowGesture::LetGo)
        );
        assert_eq!(
            asked_of_row(false, true, false, true, true, true),
            Some(RowGesture::Describe)
        );
        // The mark that asks a route over is one of them, and reads in the
        // order it is drawn: after close and info, before the dot.
        assert_eq!(
            asked_of_row(false, false, true, true, true, true),
            Some(RowGesture::Replot)
        );
        // And a plain click on the name means the filter itself.
        assert_eq!(
            asked_of_row(false, false, false, false, false, true),
            Some(RowGesture::Select)
        );
    }

    /// A click is answered once no double can still arrive, and a double
    /// takes it away
    ///
    /// [`asked_of_row`] above arbitrates the flags of one pass, and egui
    /// raises `clicked()` on the first release of a double click a whole frame
    /// before it reports the double — so the priority is handed the click
    /// alone, first, and acts on it. Which is why the click is held: a
    /// double click on a route's row is a flight to it and must not also
    /// replace what the user has picked out.
    #[test]
    fn a_click_waits_to_see_whether_it_is_half_of_a_double() {
        let ctx = egui::Context::default();
        let row = egui::Id::new("a row");
        // A pass, and what the row was told, at `time` seconds.
        let pass = |time: f64, click: bool, double: bool| {
            let mut answered = false;
            let _ = ctx.run_ui(
                egui::RawInput { time: Some(time), ..Default::default() },
                |ui| {
                    answered = settled_click(ui, row, click, double).is_some();
                },
            );
            answered
        };
        // Longer than egui's own `max_double_click_delay`.
        let window = 0.4;

        // A click alone says nothing yet, and still nothing while a double
        // could arrive.
        assert!(!pass(1.0, true, false), "the click was answered at once");
        assert!(!pass(1.05, false, false), "answered inside the window");
        assert!(pass(1.0 + window, false, false), "never answered at all");
        // And once only.
        assert!(
            !pass(1.0 + window * 2., false, false),
            "the same click was answered twice"
        );

        // The second half of a double takes the pending click with it, so the
        // window passing afterwards answers nothing.
        assert!(!pass(2.0, true, false));
        assert!(!pass(2.05, true, true), "the double read as a click");
        assert!(
            !pass(2.0 + window, false, false),
            "a double click picked its row out as well"
        );
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
            crate::ui::list::line(
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
            crate::ui::list::line(ui, egui::RichText::new("Sol"), 20., true);
        });
    }
}
