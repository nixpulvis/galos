//! How the chrome says a number, a wait or a name in the room it has
//!
//! Everything is lettered in one width, so what fits is a count of characters
//! and most of what is here is arithmetic on that.

use crate::map::route::ARROW;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Context, Ui};

/// A count with its digits grouped in threes
///
/// A population runs to eleven digits and a count of the sky to six, and
/// either is a length rather than a number until it is broken up.
pub(crate) fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (place, digit) in digits.char_indices() {
        if place > 0 && (digits.len() - place).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A wait said as a length of time
///
/// Three scales, because a plot spans all three: a route across the bubble
/// comes back in tens of milliseconds, a supercharged crossing in a couple
/// of seconds, and a proven fewest-jumps crossing in ten minutes. Two
/// significant figures at each scale — nobody waiting on a search is
/// counting microseconds, and nobody reading `612.4 s` knows how long that
/// is without doing the division themselves.
pub(crate) fn waited(took: std::time::Duration) -> String {
    let seconds = took.as_secs_f64();
    if seconds < 1. {
        format!("{} ms", took.as_millis())
    } else if seconds < 60. {
        format!("{seconds:.1} s")
    } else {
        format!("{}m {:02}s", took.as_secs() / 60, took.as_secs() % 60)
    }
}

/// What stands where a name was cut short
///
/// Two stops rather than an ellipsis, an ellipsis being one character that
/// reads as three and a name being cut to make room in the first place.
pub(super) const CUT: &str = "..";

/// Say a route in `room` characters, keeping both of its ends
///
/// A route is named for where it starts and where it ends, and either name is
/// long on its own: `SIGMA DRACONIS -> MINISTRY` is twenty six characters.
/// Cut from the right, as a widget cuts a line too long for it, what goes is
/// the end the route was plotted to reach, and every route out of one system
/// is then called the same thing.
///
/// So the room left over by the arrow is halved between them, and an end that
/// does not want its half leaves the rest to the other. What is not a route
/// is handed back whole, there being no second end to keep and whoever draws
/// it having its own way of cutting a line that does not fit.
///
/// An odd character over goes to the name that leads, that being the one read
/// first, and the two ends are otherwise given exactly as much as each other.
///
/// Counted in characters, which is a width now that everything is lettered in
/// one.
pub(crate) fn shortened(label: &str, room: usize) -> String {
    let Some((start, end)) = label.split_once(ARROW) else {
        return label.to_owned();
    };
    if label.chars().count() <= room {
        return label.to_owned();
    }

    let names = room.saturating_sub(ARROW.chars().count());
    // An end cut below a character and the mark saying it was cut is an end
    // that says nothing, and two of those either side of an arrow say only
    // that a route runs between two systems. Where it comes to that, what
    // room there is goes to the name that leads.
    if names < (CUT.chars().count() + 1) * 2 {
        return clipped(label, room);
    }

    let (start_wants, end_wants) = (start.chars().count(), end.chars().count());
    let half = names / 2;
    let (start_gets, end_gets) = if start_wants <= half {
        (start_wants, names - start_wants)
    } else if end_wants <= half {
        (names - end_wants, end_wants)
    } else {
        (names - half, half)
    };

    format!("{}{ARROW}{}", clipped(start, start_gets), clipped(end, end_gets))
}

/// Say `name` in `room` characters
///
/// Every character there is room for, cut wherever the room runs out. Backing
/// up to the word before it would read better and say less: system names are
/// told apart by their tails, `COL 285 SECTOR SC-K B22-2` from `COL 285 SECTOR
/// XY-Z A1-0`, so a name cut back to `COL 285..` is a name that no longer says
/// which one it is. A trailing space goes, being a character that says
/// nothing.
///
/// What is left ends in [`CUT`], so a name that was cut says as much. Room
/// enough for nothing but that mark is answered with as much of it as there
/// is room for: a column of them is at least a column.
fn clipped(name: &str, room: usize) -> String {
    if name.chars().count() <= room {
        return name.to_owned();
    }
    if room <= CUT.chars().count() {
        return CUT.chars().take(room).collect();
    }

    let cut = room - CUT.chars().count();
    let kept: String = name.chars().take(cut).collect();

    format!("{}{CUT}", kept.trim_end())
}

/// How wide one character of `kind` stands
///
/// Everything is lettered in one width, so a character is a measure of room
/// as much as a pixel is.
pub(crate) fn one_character(ctx: &Context, kind: egui::TextStyle) -> f32 {
    let font = kind.resolve(&ctx.global_style());
    ctx.fonts_mut(|fonts| fonts.glyph_width(&font, 'M'))
}

/// How many characters of `kind` fit in `room` pixels
///
/// Exact, and exactly what [`shortened`] is measured in, everything being
/// lettered in one width.
pub(crate) fn characters(
    ctx: &Context,
    kind: egui::TextStyle,
    room: f32,
) -> usize {
    let one = one_character(ctx, kind);
    if one <= 0. {
        return 0;
    }
    (room / one).floor().max(0.) as usize
}

/// How many characters of `kind` it takes to cover `room` pixels
///
/// [`characters`] the other way about. As many as fit stops short of the
/// room whenever the room is not a whole number of characters, which is for
/// whoever is filling a space rather than reading what is in it.
pub(crate) fn covering(
    ctx: &Context,
    kind: egui::TextStyle,
    room: f32,
) -> usize {
    let one = one_character(ctx, kind);
    if one <= 0. {
        return 0;
    }
    (room / one).ceil().max(0.) as usize
}

/// What a field holds, if it holds anything
///
/// A field clicked into and not yet typed in holds an empty string, which is
/// not something the user has said. Neither is a line of spaces.
pub(super) fn typed(field: &Option<String>) -> Option<&str> {
    field.as_deref().map(str::trim).filter(|text| !text.is_empty())
}

/// How wide a piece of body text lays out
///
/// Laid out in nothing and thrown away, which is what measuring is: the
/// width a line *wants*, before anything decides how much it gets.
pub(super) fn width(ui: &Ui, text: &str) -> f32 {
    egui::WidgetText::from(egui::RichText::new(text))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
        .size()
        .x
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ui::BAR_WIDTH;
    use std::time::Duration;

    /// A wait reads at the scale it happened on
    ///
    /// All three are real: a route across the bubble comes back in tens of
    /// milliseconds, a supercharged galactic crossing in a couple of
    /// seconds, and a proven fewest-jumps crossing of the same two ends in
    /// ten minutes. `612.4 s` is a number nobody converts in their head.
    #[test]
    fn a_wait_is_said_at_its_own_scale() {
        assert_eq!(waited(Duration::from_millis(73)), "73 ms");
        assert_eq!(waited(Duration::from_millis(2230)), "2.2 s");
        assert_eq!(waited(Duration::from_secs(612)), "10m 12s");
    }

    /// A route too long for the room keeps both of its ends
    ///
    /// Cut from the right, every route out of one system is called the same
    /// thing, and what goes is the end it was plotted to reach.
    #[test]
    fn a_route_cut_down_still_says_where_it_goes() {
        let said = shortened("SIGMA DRACONIS -> MINISTRY", 18);

        assert_eq!(said, "SIGMA.. -> MINIS..");
        assert_eq!(said.chars().count(), 18);
    }

    /// Two ends of the same length are cut to the same length
    ///
    /// Neither end is worth more than the other, so what one is given the
    /// other is given. An odd character over goes to the name that leads,
    /// that being the one read first.
    #[test]
    fn two_ends_of_a_size_are_cut_to_a_size() {
        let both = shortened("COL 1232312312 -> COL 3211231231", 22);
        assert_eq!(both, "COL 123.. -> COL 321..");

        let odd = shortened("COL 1232312312 -> COL 3211231231", 21);
        assert_eq!(odd, "COL 123.. -> COL 32..");
    }

    /// What fits is left alone, route or not
    #[test]
    fn what_fits_is_said_whole() {
        assert_eq!(shortened("SOL -> WOLF 359", 20), "SOL -> WOLF 359");
        assert_eq!(shortened("Alliance of Sol", 4), "Alliance of Sol");
    }

    /// An end that does not want its half leaves the rest to the other
    ///
    /// Half each is the fair share and not the useful one: a route from SOL
    /// has room going spare at one end and a name being cut at the other.
    #[test]
    fn an_end_with_room_to_spare_gives_it_to_the_other() {
        let said = shortened("SOL -> COL 285 SECTOR SC-K B22-2", 20);

        assert_eq!(said, "SOL -> COL 285 SEC..");
        assert_eq!(said.chars().count(), 20);
    }

    /// A name is cut where the room runs out, word or no word
    ///
    /// Systems are told apart by the tails of their names, so every character
    /// there is room for is worth having. Backed up to the word before it,
    /// `COL 285 SECTOR SC-K B22-2` and `COL 285 SECTOR XY-Z A1-0` are the same
    /// row twice.
    ///
    /// A trailing space goes with the cut, being a character that says
    /// nothing.
    #[test]
    fn a_name_is_cut_where_the_room_runs_out() {
        assert_eq!(clipped("COL 285 SECTOR SC-K B22-2", 12), "COL 285 SE..");
        assert_eq!(clipped("COL 285 SECTOR", 9), "COL 285..");
        assert_eq!(clipped("SIGMA DRACONIS", 8), "SIGMA..");
        assert_eq!(clipped("MINISTRY", 6), "MINI..");
    }

    /// Room for nothing but the mark is answered with the mark
    ///
    /// A row of them is at least a row, where a name cut to no characters at
    /// all is a gap the reader has to work out the meaning of.
    #[test]
    fn a_name_with_no_room_is_all_mark() {
        assert_eq!(clipped("MINISTRY", 2), "..");
        assert_eq!(clipped("MINISTRY", 1), ".");
        assert_eq!(clipped("MINISTRY", 0), "");
    }

    /// The count fits the bar at the size the sky is heading for
    ///
    /// Both numbers grow with what has been synced, and the line has to hold
    /// them on one row: wrapped, it is a line that moves the rows under it
    /// about as the user flies. Seven digits either side comes to 235 of the
    /// 325 the bar is wide, so the sky can grow well past millions of systems
    /// before the line has nowhere left to grow into.
    #[test]
    fn the_count_fits_the_bar_at_millions() {
        let ctx = crate::testing::context();
        let said = format!(
            "{} of {} in spyglass",
            thousands(1_234_567),
            thousands(7_654_321)
        );

        let mut width = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            width = egui::WidgetText::from(egui::RichText::new(&said).weak())
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                )
                .size()
                .x;
        });

        assert!(
            width <= BAR_WIDTH,
            "{said:?} wants {width}px of the {BAR_WIDTH} there are"
        );
    }

    /// A number short enough to read is left as it is
    ///
    /// Which is most of what is handed over: the systems with nobody living
    /// in them far outnumber the inhabited ones, and a sky with a handful in
    /// reach is a handful.
    #[test]
    fn small_counts_are_left_alone() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(999), "999");
    }

    /// Longer ones are broken into threes from the right
    ///
    /// From the right, so that the leading group is whatever is left over
    /// rather than the number being padded to fit.
    #[test]
    fn long_counts_are_grouped_from_the_right() {
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(22_780), "22,780");
        assert_eq!(thousands(999_999), "999,999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    /// The largest counts on record still read
    ///
    /// The most populous systems run to eleven digits, which is the longest
    /// this is ever handed.
    #[test]
    fn the_largest_counts_are_grouped() {
        assert_eq!(thousands(22_780_919_531), "22,780,919,531");
    }

    /// A separator never leads or trails
    ///
    /// The grouping is decided per digit from how many follow it, so a count
    /// whose length is a multiple of three is where a stray leading comma
    /// would show up.
    #[test]
    fn grouping_never_leads_or_trails() {
        for count in [1u64, 100, 1_000, 100_000, 1_000_000] {
            let grouped = thousands(count);
            assert!(!grouped.starts_with(','), "{grouped} leads with one");
            assert!(!grouped.ends_with(','), "{grouped} trails one");
        }
    }

    /// A field clicked into and left alone holds nothing
    ///
    /// Egui hands back an empty string for it, and taking that as an answer
    /// is what has a form telling the user off for having touched it.
    #[test]
    fn a_field_only_typed_into_holds_anything() {
        assert_eq!(typed(&None), None);
        assert_eq!(typed(&Some(String::new())), None);
        assert_eq!(typed(&Some("   ".to_owned())), None);
        assert_eq!(typed(&Some(" Sol ".to_owned())), Some("Sol"));
    }
}
