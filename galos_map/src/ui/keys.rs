//! What the keyboard asks of the chrome
//!
//! The bindings that open and put away what is drawn over the map: the bar's
//! three questions, the bindings window, and the escape that puts all of it
//! away. What the keyboard asks of the map itself is [`crate::map::keys`], and
//! the rules both read — what a chord is, whether a field has the caret — are
//! [`crate::input`]'s.

use super::bar::{AskMode, BarFields};
use super::{KeysOpen, Panes};
use crate::input::{Keyboard, bare, shifted};
use crate::map::schedule::MapSet;
use bevy::prelude::*;

pub(crate) fn plugin(app: &mut App) {
    // `shut_search` before `toggle_keys`, both reading the one escape: the
    // form stands down while the bindings window is up, so it has to be asked
    // before the window takes itself down. Run the other way round, the
    // window would shut and the form would then see it closed and shut too,
    // which is one press putting away two things.
    app.add_systems(
        Update,
        (open_search, shut_search, toggle_keys).chain().in_set(MapSet::Search),
    );
}

/// Show or hide what the keys do
///
/// Two ways in, and both are where a reader looks: `F1` is help on every
/// desktop there is, and `?` is help everywhere a page has a keyboard. Shut
/// by either again, by the window's own mark, and by escape.
///
/// `?` is shift and the slash key, where the bare slash puts the caret in the
/// search box: the same key, and the modifier is the whole of what tells the
/// two apart. Asked about before [`open_search`] runs would make no
/// difference — that one wants the slash bare — but the shift is checked here
/// all the same rather than left to the order the systems happen to run in.
fn toggle_keys(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    mut open: ResMut<KeysOpen>,
) {
    if keyboard.typing {
        return;
    }

    let helped = keys.just_pressed(KeyCode::F1) && bare(&keys);
    let asked = keys.just_pressed(KeyCode::Slash) && shifted(&keys);
    if helped || asked {
        open.0 = !open.0;
    }
    // As it shuts the form, and for the same reason: escape is where a reader
    // looks for the way out of something they opened. Only where it is open,
    // so an escape meant for the form is not spent here.
    if open.0 && keys.just_pressed(KeyCode::Escape) && bare(&keys) {
        open.0 = false;
    }
}

/// Put the caret in the bar's box, asking whichever question was reached for
///
/// One box puts three questions and a key reaches each of them, so that a
/// mode is not something only a tab knows about: `/` searches for a system,
/// shift-F filters, shift-R plots a route. See [`crate::ui::bar::AskMode`].
///
/// The search has two ways in. `/` is where a reader who came from a browser
/// or an editor will look for it, and shift-S is under a hand already resting
/// on the pan keys. Neither is the bare S, which pans the map back.
///
/// The other two are shifted for the same reason: bare F and R zoom the map,
/// and every letter under that hand is spoken for. Shift is the modifier the
/// map already reads on its own — see [`shifted`] — and F and R are the
/// letters of the things they open.
fn open_search(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    mut bar: ResMut<BarFields>,
) {
    if keyboard.typing {
        return;
    }

    let slashed = keys.just_pressed(KeyCode::Slash) && bare(&keys);
    let spelled = keys.just_pressed(KeyCode::KeyS) && shifted(&keys);
    if slashed || spelled {
        bar.open(AskMode::System);
    }
    if keys.just_pressed(KeyCode::KeyF) && shifted(&keys) {
        bar.open(AskMode::Filter);
    }
    if keys.just_pressed(KeyCode::KeyR) && shifted(&keys) {
        bar.open(AskMode::Route);
    }
}

/// Put away whatever the chrome has open
///
/// Escape, which is where a reader looks for the way out of something they
/// have opened. What was typed is left standing for whenever the form is
/// opened again, the form being shut rather than the question thrown out.
///
/// Every pane, and through [`crate::ui::Panes`], which is the same code a
/// click on empty sky goes through — see
/// [`crate::map::selection::nothing_clicked`]. A key that knew the bar
/// from the clock would be a third place to teach about the fourth thing
/// that drops out of the chrome. Taken one at a time to begin with, on the
/// argument that one gesture means one thing; what that came to in the hand
/// was a key pressed twice to clear a screen it looked like it had cleared.
///
/// The bindings window is the exception, and takes itself down first: it is
/// read over everything, including a form left standing while the reader
/// looks something up. See [`toggle_keys`], which is the other half of what
/// this says about the one key.
///
/// The one binding that answers while a field is being typed into, and it has
/// to: a caret in a field is the state this exists to undo. Nothing is lost by
/// its doing so, an escape being no part of any name.
fn shut_search(
    keys: Res<ButtonInput<KeyCode>>,
    open: Res<KeysOpen>,
    mut panes: Panes,
) {
    if open.0 {
        return;
    }
    if !(keys.just_pressed(KeyCode::Escape) && bare(&keys)) {
        return;
    }

    panes.shut_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// How long a frame lasts here
    ///
    /// Said rather than measured. What a key is worth is a rate, and a clock
    /// running at the speed of the machine would answer differently every time
    /// it was asked.
    const STEP: f32 = 1. / 60.;

    /// A world with the keyboard and the clock the bindings read, and nothing
    /// running in it
    fn world() -> App {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<Time<Real>>();
        app.init_resource::<Keyboard>();
        app
    }

    /// Run one frame with `keys` down and every other key up
    ///
    /// The input is cleared first, as bevy's own does at the top of a frame:
    /// a toggle reads `just_pressed`, which stands until something clears it,
    /// and a key left marked would be read as pressed again every frame.
    fn frame(app: &mut App, keys: &[KeyCode]) {
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs_f32(STEP));

        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.clear();
        let down: Vec<KeyCode> = input.get_pressed().copied().collect();
        for key in down {
            if !keys.contains(&key) {
                input.release(key);
            }
        }
        for key in keys {
            input.press(*key);
        }

        app.update();
    }

    /// Press `keys` and let them up again
    ///
    /// Two frames, so that a test can press the same key twice.
    fn pressed(app: &mut App, keys: &[KeyCode]) {
        frame(app, keys);
        frame(app, &[]);
    }

    /// Put a name in a field and the caret in it
    ///
    /// A field being typed into holds the focus as well, so both are said. The
    /// two only ever part company the other way round.
    fn type_a_name(app: &mut App) {
        *app.world_mut().resource_mut::<Keyboard>() =
            Keyboard { typing: true, focused: true };
    }

    /// A world where the search box can be asked for
    fn barred() -> App {
        let mut app = world();
        app.init_resource::<BarFields>();
        app.init_resource::<KeysOpen>();
        app.init_resource::<super::super::ClockControl>();
        app.add_systems(
            Update,
            (open_search, shut_search, toggle_keys).chain(),
        );
        app
    }

    /// Whether the search box has been asked for
    fn opening(app: &App) -> bool {
        app.world().resource::<BarFields>().opening
    }

    /// Whether the form has been asked to be put away
    fn shutting(app: &App) -> bool {
        app.world().resource::<BarFields>().shutting
    }

    /// Which question the box has been asked to put
    fn asking(app: &App) -> Option<AskMode> {
        app.world().resource::<BarFields>().asking
    }

    /// `/` asks for the search box
    #[test]
    fn a_slash_asks_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::Slash]);

        assert!(opening(&app));
    }

    /// So does shift-S
    #[test]
    fn a_shifted_s_asks_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::ShiftLeft, KeyCode::KeyS]);

        assert!(opening(&app));
    }

    /// A bare S does not, that being how the map is panned back
    #[test]
    fn a_bare_s_does_not_ask_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::KeyS]);

        assert!(!opening(&app));
    }

    /// A shifted slash is a question mark rather than a binding
    #[test]
    fn a_shifted_slash_does_not_ask_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::ShiftLeft, KeyCode::Slash]);

        assert!(!opening(&app));
    }

    /// Each key asks its own question of the one box
    ///
    /// The mode has to be settled by the key rather than waited for, since
    /// the pass that puts the caret in draws the field the caret belongs in:
    /// one frame asking the wrong question is a keystroke typed into the
    /// wrong field.
    #[test]
    fn each_key_asks_its_own_question() {
        for (chord, meant) in [
            (&[KeyCode::Slash][..], AskMode::System),
            (&[KeyCode::ShiftLeft, KeyCode::KeyS], AskMode::System),
            (&[KeyCode::ShiftLeft, KeyCode::KeyF], AskMode::Filter),
            (&[KeyCode::ShiftLeft, KeyCode::KeyR], AskMode::Route),
        ] {
            let mut app = barred();

            pressed(&mut app, chord);

            assert_eq!(asking(&app), Some(meant), "{chord:?}");
            assert!(opening(&app), "{chord:?} asked for no caret");
        }
    }

    /// Bare F and R are the zoom, not the two questions
    ///
    /// Which is why those two want shift. Every letter under that hand is
    /// spoken for, and a key that opened a form mid-zoom would take the
    /// keyboard away from the map.
    #[test]
    fn a_bare_f_or_r_asks_nothing_of_the_box() {
        for key in [KeyCode::KeyF, KeyCode::KeyR] {
            let mut app = barred();

            pressed(&mut app, &[key]);

            assert_eq!(asking(&app), None, "{key:?}");
            assert!(!opening(&app), "{key:?}");
        }
    }

    /// Nor does a slash typed into a field
    ///
    /// Which is the whole of why the field is asked about: the box that would
    /// be opened is the box the slash is being typed into.
    #[test]
    fn a_slash_typed_into_a_field_is_not_a_binding() {
        let mut app = barred();
        type_a_name(&mut app);

        pressed(&mut app, &[KeyCode::Slash]);

        assert!(!opening(&app));
    }

    /// Ask a question of the box, as a click into it or a key does
    fn ask(app: &mut App) {
        app.world_mut().resource_mut::<BarFields>().open(AskMode::System);
    }

    /// An escape asks for the form to be put away
    #[test]
    fn an_escape_puts_the_form_away() {
        let mut app = barred();
        ask(&mut app);

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(shutting(&app));
    }

    /// And answers while a name is being typed, alone among the bindings
    ///
    /// A caret in a field is the state it exists to undo, so standing down for
    /// one would leave it unable to do the only thing it does. Nothing is lost
    /// by its answering, an escape being no part of any name.
    #[test]
    fn an_escape_answers_while_a_name_is_being_typed() {
        let mut app = barred();
        ask(&mut app);
        type_a_name(&mut app);

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(shutting(&app));
    }

    /// And stands down while the bindings are being read
    ///
    /// The window is read over everything, a form among it, so it is the one
    /// thing an escape takes before the panes. Both answered the one press
    /// before this: the window shut, as it should, and the form collapsed
    /// behind it — a form the user had left standing while they looked
    /// something up, and no way to put away only the window.
    #[test]
    fn an_escape_meant_for_the_bindings_is_not_spent_on_the_form() {
        let mut app = barred();
        ask(&mut app);

        pressed(&mut app, &[KeyCode::F1]);
        assert!(helping(&app), "the bindings did not open");

        pressed(&mut app, &[KeyCode::Escape]);
        assert!(!helping(&app), "the escape did not shut the bindings");
        assert!(!shutting(&app), "and it put the form away as well");

        // With the window down, the next one is the form's as it always was.
        pressed(&mut app, &[KeyCode::Escape]);
        assert!(shutting(&app), "the form no longer answers an escape");
    }

    /// Whether the scrubber is out
    fn scrubbing(app: &App) -> bool {
        app.world().resource::<super::super::ClockControl>().out
    }

    /// Put the scrubber out, as a click on the reading does
    fn scrub(app: &mut App) {
        app.world_mut().resource_mut::<super::super::ClockControl>().out = true;
    }

    /// An escape puts the scrubber away where no form is out
    ///
    /// The clock's rail is opened by a click on the reading and was shut only
    /// by a second one. Everything else the map opens is put away by the key
    /// a reader looks for, and a panel that is the one exception is a panel
    /// nobody can dismiss without hunting for the thing they clicked.
    #[test]
    fn an_escape_puts_the_scrubber_away() {
        let mut app = barred();
        scrub(&mut app);

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(!scrubbing(&app));
    }

    /// And puts away everything that is out, in the one press
    ///
    /// Taken one at a time to begin with, the form first, on the argument
    /// that one gesture means one thing. In the hand that was a key pressed
    /// twice to clear a screen it looked like it had cleared the first time.
    #[test]
    fn an_escape_puts_every_pane_away() {
        let mut app = barred();
        scrub(&mut app);
        ask(&mut app);

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(shutting(&app), "the form was left standing");
        assert!(!scrubbing(&app), "the scrubber was left standing");
    }

    /// A world where the bindings can be asked for
    fn helped() -> App {
        let mut app = world();
        app.init_resource::<KeysOpen>();
        app.add_systems(Update, toggle_keys);
        app
    }

    /// Whether the bindings are being read
    fn helping(app: &App) -> bool {
        app.world().resource::<KeysOpen>().0
    }

    /// F1 shows what the keys do, and hides it again
    #[test]
    fn f1_shows_the_bindings_and_hides_them() {
        let mut app = helped();

        pressed(&mut app, &[KeyCode::F1]);
        assert!(helping(&app));

        pressed(&mut app, &[KeyCode::F1]);
        assert!(!helping(&app), "the same key did not put them away");
    }

    /// So does a question mark, which is a shifted slash
    ///
    /// The bare slash is the search box, so the two bindings share a key and
    /// the modifier is the whole of what tells them apart: a reader asking
    /// for help must not land in the search box, and one reaching for the
    /// search box must not be handed a reference.
    #[test]
    fn a_question_mark_shows_the_bindings_and_a_bare_slash_does_not() {
        let mut app = helped();

        pressed(&mut app, &[KeyCode::Slash]);
        assert!(!helping(&app), "a bare slash is the search box");

        pressed(&mut app, &[KeyCode::ShiftLeft, KeyCode::Slash]);
        assert!(helping(&app));
    }

    /// An escape puts them away, and only while they are up
    ///
    /// Where a reader looks for the way out of anything they opened. Spent
    /// here while they are shut, it would be an escape the search form never
    /// saw — the two answer the same key and only one of them is open.
    #[test]
    fn an_escape_puts_the_bindings_away() {
        let mut app = helped();

        pressed(&mut app, &[KeyCode::F1]);
        pressed(&mut app, &[KeyCode::Escape]);
        assert!(!helping(&app));

        // Shut already: nothing to do, and nothing done.
        pressed(&mut app, &[KeyCode::Escape]);
        assert!(!helping(&app));
    }

    /// And a name being typed is not a reader asking for help
    #[test]
    fn typing_a_name_does_not_show_the_bindings() {
        let mut app = helped();
        type_a_name(&mut app);

        pressed(&mut app, &[KeyCode::F1]);

        assert!(!helping(&app));
    }

    /// It does not ask for the box it is putting away
    ///
    /// The two are one key apart on the same resource, and asking for both in
    /// a frame would leave the form to be shut and opened at once.
    #[test]
    fn an_escape_does_not_ask_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(!opening(&app));
    }

    /// Nor does opening the box ask for it to be put away
    #[test]
    fn a_slash_does_not_put_the_form_away() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::Slash]);

        assert!(!shutting(&app));
    }
}
