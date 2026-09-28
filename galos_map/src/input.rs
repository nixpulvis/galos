//! Who the pointer and the keyboard belong to: the map, or the chrome over it
//!
//! Only the chrome knows which of a window's pixels are its own and whether a
//! field has the caret, so it answers here and the map reads the answers. A map
//! with no chrome over it reads the defaults — nothing over it, nobody typing —
//! and has the whole of the pointer and the keyboard to itself.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// Stand the answers up, so a map with no chrome over it can ask them.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PointerOverUi>();
    app.init_resource::<Keyboard>();
    app.init_resource::<PressOwner>();
}

/// The button that answers for whatever is under the pointer
///
/// Picking knows it as [`PointerButton::Primary`], and [`ButtonInput`] knows
/// it by where it sits, so the two names are put together here. What a press
/// selects and what a press clears are then the same button by construction
/// rather than by two files happening to agree.
pub const PRIMARY: MouseButton = MouseButton::Left;

/// Whether the pointer is busy with the UI
///
/// Only the UI knows which of a window's pixels are its own, so it answers
/// here rather than the map guessing from rectangles it would have to be told
/// about.
///
/// Where the pointer is now, which is the question a wheel asks: a scroll
/// belongs to no press and so has no owner to be asked about. What a press
/// belongs to is [`PressOwner`], and everything weighing a click or a drag asks
/// that instead.
///
/// Egui lays out during its own pass, so this is what the last frame's layout
/// concluded. A wheel turned over a pane that was not there last frame turns
/// the map as well, which is a pane the user has only just opened.
#[derive(Resource, Default)]
pub(crate) struct PointerOverUi(pub(crate) bool);

/// What the chrome has taken of the keyboard
///
/// The map is driven by bare keys and so is most of what a field wants, so the
/// two have to be told apart. See [`crate::map::keys`], which is the whole of what
/// reads this.
///
/// Two questions rather than one, because the chrome takes the keyboard at two
/// strengths. A field being typed into takes every letter. Anything holding the
/// focus takes only space and enter, which egui reads as a click on whatever
/// holds it.
///
/// Settled at the end of the chrome's own pass and read by the next frame's
/// [`crate::map::schedule::MapSet::Search`], as [`PointerOverUi`] is.
#[derive(Resource, Default)]
pub(crate) struct Keyboard {
    /// Whether a field is being typed into
    ///
    /// A text field alone. Egui goes on holding a focus wherever tab last
    /// reached, a checkbox on the settings pane among them, and a checkbox does
    /// nothing with a letter. Reading [`Keyboard::focused`] instead would leave
    /// every letter the map is driven by dead until the focus was let go of.
    pub(crate) typing: bool,
    /// Whether anything in the chrome holds the focus
    ///
    /// What a binding on space or enter has to read instead. Egui reads either
    /// of those as a click on whatever holds the focus, so the chrome answers
    /// them before the map does, and a control tabbed onto and left holding it
    /// would be clicked again by every press of the key that flies the camera.
    ///
    /// Wider than [`Keyboard::typing`] only while the user is stepping the
    /// chrome by keyboard: a click grants no focus, so nothing else puts it on
    /// a control that is not a field.
    pub(crate) focused: bool,
}

/// Whose a press is
///
/// Decided once, when the button goes down, and held until it comes up. The
/// pointer is doing one thing at a time and the thing it is doing belongs to
/// somebody: a drag that began on a slider is the slider's for as long as it
/// lasts, wherever the pointer wanders, and a press that shut the bar's form
/// is the form's even though it landed on the sky.
/// Never named outside this module. What the rest of the map asks is whose a
/// press is, and every answer to that is a `bool` on [`PressOwner`] or
/// [`Gesture`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Owner {
    /// The pointer was over a control, or the press was spent on one
    Ui,
    /// The map's to answer
    Map,
}

/// Who the press under way belongs to
///
/// Egui draws from `PostUpdate`, after every system that answers a click, so
/// what the UI made of a press is a frame behind whoever asks. Settling it
/// once at the press rather than asking afresh at the release is what makes
/// the lateness harmless: a release is a frame after its own press at worst,
/// by which time this has been written.
///
/// Reached through [`Gesture`] rather than read directly, that being where the
/// one case this cannot answer straight away is handled.
#[derive(Resource, Default)]
pub(crate) struct PressOwner {
    /// Whose the press under way is, while a button is down
    owner: Option<Owner>,
    /// Whose a press was that came up in the same frame it went down
    ///
    /// A frame long enough to hold a whole click puts the map's reading of it
    /// before the UI's, so whose it was is news that has to keep until the
    /// next frame. Standing for one frame and no longer.
    ///
    /// A second whole click in that next frame takes its place and the first
    /// goes unanswered. Two entire clicks inside two frames is a frame rate
    /// with troubles this cannot help with.
    carried_over: Option<Owner>,
}

impl PressOwner {
    /// Every button the map answers to
    ///
    /// One owner for the pointer rather than one per button. The pointer is
    /// doing one thing, and a press landing while another is already down is
    /// part of whatever that was.
    const BUTTONS: [MouseButton; 3] =
        [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

    /// Settle who the pointer belongs to, the UI having now spoken
    ///
    /// `wanted` is whether the UI took this press: the pointer was over a
    /// control, or the press was spent shutting something. Called at the end
    /// of the UI's own pass, that being the first moment either is known.
    ///
    /// Wants reaching every frame. Nothing else clears an owner, so a frame
    /// that draws no UI at all leaves the last press held, and a press held
    /// after the button came up reads as a drag of the map that never ends
    /// and as a click on every release after it. [`crate::ui::chrome`] gives
    /// up before here when there is no egui context to draw into, which is a
    /// map with no window rather than a map with a stuck pointer, so this is
    /// left as the simpler arrangement of the two. Should it ever be seen,
    /// the fix is to settle from a system of its own, reading what the UI
    /// wanted out of a resource rather than off the end of drawing.
    pub(crate) fn settle(
        &mut self,
        buttons: &ButtonInput<MouseButton>,
        wanted: bool,
    ) {
        // Last frame's, which has now been read by everything that reads it.
        self.carried_over = None;

        let began = buttons.any_just_pressed(Self::BUTTONS);
        if began && self.owner.is_none() {
            self.owner = Some(if wanted { Owner::Ui } else { Owner::Map });
        }

        if !buttons.any_pressed(Self::BUTTONS) {
            if began && buttons.just_released(PRIMARY) {
                self.carried_over = self.owner;
            }
            self.owner = None;
        }
    }

    /// Whether the press under way is the UI's
    ///
    /// The question for whoever cannot wait to be told. A press nobody owns
    /// yet answers no: picking reports a click before the UI has settled
    /// whose the press was, and a star that cannot be picked out on a slow
    /// map would be a worse answer than one picked out during a gesture the
    /// UI turned out to want.
    pub(crate) fn taken_by_ui(&self) -> bool {
        self.owner == Some(Owner::Ui)
    }
}

/// What the pointer has just done, and whether it was the map's to answer
///
/// The one question every system weighing a click asks, so that none of them
/// works out an answer of its own from the button and where the pointer was.
/// Both halves are needed together: the button says what happened this frame
/// and [`PressOwner`] says whose it was.
#[derive(SystemParam)]
pub(crate) struct Gesture<'w> {
    buttons: Res<'w, ButtonInput<MouseButton>>,
    press: Res<'w, PressOwner>,
}

impl Gesture<'_> {
    /// Whether the map is being dragged
    ///
    /// False for the first frame of a drag, the UI not having said whose it
    /// is until the end of that frame. A frame of a map that has not started
    /// turning yet, against a frame of one that turns under a press meant for
    /// a slider.
    pub(crate) fn dragging_map(&self) -> bool {
        self.press.owner == Some(Owner::Map)
    }

    /// Whether `button` is down
    ///
    /// Which of them is being dragged with, once [`Self::dragging_map`] has
    /// said the drag is the map's at all. Offered here so that asking takes
    /// one thing rather than a system holding its own copy of the input
    /// beside this, which would be two readings of the same buttons sitting
    /// where they could be told apart.
    pub(crate) fn pressed(&self, button: MouseButton) -> bool {
        self.buttons.pressed(button)
    }

    /// Whether a click the map owns has just finished
    ///
    /// On the release, where the press landed in an earlier frame and the
    /// owner is already standing. A frame holding the whole click answers a
    /// frame later, through [`PressOwner::carried_over`], which is the one
    /// place that
    /// wait is spelled out.
    pub(crate) fn on_map(&self) -> bool {
        if self.buttons.just_released(PRIMARY) {
            return self.press.owner == Some(Owner::Map);
        }
        self.press.carried_over == Some(Owner::Map)
    }
}

/// The keys that turn a press into a chord rather than a binding
///
/// Shift is not among them. Two bindings want it of their own — the `S` that
/// opens the search and the `?` that opens the bindings window — so it is
/// asked about separately by [`bare`] and [`shifted`].
const CHORDING: [KeyCode; 6] = [
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
    KeyCode::SuperLeft,
    KeyCode::SuperRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
];

const SHIFT: [KeyCode; 2] = [KeyCode::ShiftLeft, KeyCode::ShiftRight];

/// Whether a press is on its way somewhere other than the map
pub(crate) fn chorded(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed(CHORDING)
}

/// Whether a key is being pressed on its own
pub(crate) fn bare(keys: &ButtonInput<KeyCode>) -> bool {
    !chorded(keys) && !keys.any_pressed(SHIFT)
}

/// Whether a key is being pressed with shift and nothing else
pub(crate) fn shifted(keys: &ButtonInput<KeyCode>) -> bool {
    !chorded(keys) && keys.any_pressed(SHIFT)
}

/// Whether a press means "and this as well"
///
/// Held down, a modifier gathers systems up rather than replacing what is
/// held, and lets go of one already held, so the same gesture builds a set
/// and takes it apart. Any of the three, and both sides of each. Which one
/// means "as well as that one" is a matter of what the user came from:
/// control on Windows and Linux, command on macOS. Shift is offered beside
/// them because it is the one no platform reads as asking for something
/// else. The chrome's rows read the same three off egui's own modifiers.
pub(crate) fn gathering(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
    ])
}
