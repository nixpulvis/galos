//! The settings pane, and the gear that slides it out
//!
//! What the map lets a user set once and leave alone: the spyglass, the
//! names, the grid, the two views and how faintly the filters dim. Drawn by
//! [`crate::ui::chrome`] before anything else, since the gear stands wherever
//! the pane has reached.

use crate::map::bodies::Clock;
use crate::map::bodies::spawn::ShowOrbits;
use crate::map::filter::DimTo;
use crate::map::galaxy::Spyglass;
use crate::map::galaxy::fetch::Poll;
use crate::map::galaxy::spawn::{
    ColorBy, ShowNames, StarExposure, StarProfile,
};
use crate::map::grid::{Bright, RulerUnit, ShowGrid, ShowMiddle, ShowPicked};
use crate::map::labels::{NameLimit, NameRadius, ShowBodyNames};
use crate::map::paint::glow::FieldExposure;
use crate::map::paint::sizing::{ScalePopulation, View};
use crate::ui::widgets::{VALUE_WIDTH, check, fill_width, value_box};
use crate::ui::{ClockControl, FIELD_GAP, GEAR_ROOM, MARGIN, ShowClock, zone};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Context, Response, Ui};
use galos_photometry::psf::ProfileKind;

/// How wide the settings pane stands when it is out
const PANE_WIDTH: f32 = 240.;

/// How tall the gear is drawn
const GEAR_SIZE: f32 = 18.;

/// How much of the value a drag on the box beside a radius is worth
///
/// A fraction of the number itself rather than a distance, since a radius runs
/// over four decades and a drag has one speed: half a percent per pixel moves
/// 5 to 6 as readily as 50,000 to 100,000, where a fixed speed does one or the
/// other and not both.
///
/// Far finer than the rail beside it, which is the point of having both. A
/// logarithmic rail spends those four decades over its own width, which comes
/// to about seven percent of the value for every pixel of it, so the rail
/// reaches anywhere and settles on nothing. This is the instrument for
/// settling, and it is worth roughly a tenth of what the rail is.
const RADIUS_DRAG: f32 = 0.005;

/// A radius in light years, on a log slider with a box beside it
///
/// One rail, logarithmic, since the range runs over five orders of magnitude
/// and a linear one would spend nearly all of itself between ten thousand
/// light years and a hundred thousand, which is a distance nobody sets, while
/// leaving no purchase at all down where a single light year is a real
/// distance. A logarithmic rail gives every decade the same room.
///
/// Exactness is the box's business, not the rail's. A pixel near the top of
/// the rail is worth hundreds of light years however it is scaled, so a number
/// that has to be exact is typed rather than dragged to.
///
/// `ceiling` bounds what can be asked for, and only that. A value already
/// above it is shown held down to it and left alone underneath, so a ceiling
/// that moves cannot quietly rewrite a setting: names asked for out to twenty
/// light years stay asked for out to twenty when the spyglass is drawn in to
/// five, and are back at twenty when it opens again. Writing the held figure
/// back instead loses the asking the first frame the ceiling drops under it,
/// with nothing to say it happened and no way to get it back but to ask again.
fn radius_slider(ui: &mut Ui, radius: &mut f32, ceiling: f32) -> Response {
    let ceiling = ceiling.clamp(Spyglass::FLOOR, Spyglass::CEILING);
    // The galaxy's own outer bound does not move, so a radius past it is out
    // of range rather than merely out of reach, and is corrected once and
    // kept.
    //
    // There is no such bound underneath. The reach follows the camera all the
    // way in (see [`crate::map::galaxy::reach_with_camera`]), so a radius under
    // the rail's own least is a real setting and not a mistake to be
    // corrected: writing the rail's least back would hold the sky within a
    // thousandth of a light year open every frame this pane is drawn, and put
    // the neighbours of a system flown into back on the map. Shown held to the
    // rail and left alone underneath, as a radius over the ceiling is.
    *radius = radius.min(Spyglass::CEILING);
    // Read before the rail borrows it, and the reason it is read at all.
    let speed = (*radius * RADIUS_DRAG).max(f32::EPSILON) as f64;
    fill_width(ui, VALUE_WIDTH);

    // What the widgets work on. They hold whatever they are given inside the
    // range, so this is the copy that gets held rather than the setting.
    let mut asked = radius.clamp(Spyglass::FLOOR, ceiling);

    let response = ui
        .horizontal(|ui| {
            let rail = ui.add(
                egui::Slider::new(&mut asked, Spyglass::FLOOR..=ceiling)
                    .logarithmic(true)
                    .show_value(false),
            );
            let box_ = value_box(
                ui,
                egui::DragValue::new(&mut asked)
                    .range(Spyglass::FLOOR..=ceiling)
                    .speed(speed),
            );
            rail.union(box_)
        })
        .inner;

    // Only where the figure was actually asked for. Anything else is the
    // ceiling having moved, which is not an answer to the question.
    if response.changed() {
        *radius = asked;
    }

    response
}

/// Everything the settings pane sets
///
/// One parameter rather than nine. A system may take only sixteen, and these
/// are all the same thing: what the pane is a pane of.
#[derive(SystemParam)]
pub(crate) struct Settings<'w> {
    spyglass: ResMut<'w, Spyglass>,
    view: ResMut<'w, View>,
    color_by: ResMut<'w, ColorBy>,
    population_scale: ResMut<'w, ScalePopulation>,
    star_exposure: ResMut<'w, StarExposure>,
    field_exposure: ResMut<'w, FieldExposure>,
    star_profile: ResMut<'w, StarProfile>,
    show_names: ResMut<'w, ShowNames>,
    poll: ResMut<'w, Poll>,
    name_radius: ResMut<'w, NameRadius>,
    name_limit: ResMut<'w, NameLimit>,
    show_orbits: ResMut<'w, ShowOrbits>,
    pub(super) clock: ResMut<'w, Clock>,
    pub(super) clock_control: ResMut<'w, ClockControl>,
    pub(super) show_clock: ResMut<'w, ShowClock>,
    show_body_names: ResMut<'w, ShowBodyNames>,
    show_grid: ResMut<'w, ShowGrid>,
    unit: ResMut<'w, RulerUnit>,
    show_middle: ResMut<'w, ShowMiddle>,
    show_picked: ResMut<'w, ShowPicked>,
    bright: ResMut<'w, Bright>,
}

/// What the settings pane holds, section by section
///
/// Drawn into the pane [`settings_pane`] slides out. Everything it sets is
/// reached through [`Settings`] but the dimming, which is the filters' own and
/// is handed over from [`FilterBar`](crate::ui::bar::filter::FilterBar) alone.
///
/// Every setting a widget works on is handed to it through [`edited`], so a
/// pane that is open and left alone marks nothing changed.
pub(super) fn settings_body(
    ui: &mut Ui,
    settings: &mut Settings,
    dim: &mut ResMut<DimTo>,
) {
    heading(ui, "Spyglass", false);
    // The spyglass is now a bound on the LoD walk rather than a source of
    // its own: enabled, the walk is clamped to the reach and only the near
    // sky draws; disabled, the whole sky draws, thinned by the level of
    // detail alone. What is under it only settles where that bound falls,
    // so it nests under the toggle the way the other sections nest theirs.
    // "Enable" is the spyglass's `clear`: to bound the view is to clear
    // away what the reach does not hold.
    edited(
        &mut settings.spyglass,
        |x| &mut x.clear,
        |on| check(ui, on, "Enable", "Only draw systems near the camera"),
    );
    if settings.spyglass.clear {
        ui.indent("spyglass", |ui| {
            edited(
                &mut settings.spyglass,
                |x| &mut x.follow_camera,
                |on| {
                    check(
                        ui,
                        on,
                        "Follow Camera",
                        "Set the radius from the camera's zoom",
                    )
                },
            );
            ui.add_space(FIELD_GAP);
            titled(
                ui,
                "Radius (Ly)",
                "How far from the camera to draw systems",
            );
            // Greyed while the camera sets it: dragging it would be
            // overwritten on the next frame, and a control that springs
            // back is worse than one that says it is not yours to move.
            ui.add_enabled_ui(!settings.spyglass.follow_camera, |ui| {
                edited(
                    &mut settings.spyglass,
                    |x| &mut x.radius,
                    |radius| radius_slider(ui, radius, Spyglass::CEILING),
                );
            });
            // The camera cannot both be told where to stand and be asked
            // where it is standing, so the one that reads the camera hides
            // the one that writes it.
            if !settings.spyglass.follow_camera {
                ui.add_space(FIELD_GAP);
                edited(
                    &mut settings.spyglass,
                    |x| &mut x.lock_camera,
                    |on| {
                        check(
                            ui,
                            on,
                            "Lock Camera",
                            "Stop the camera leaving the radius",
                        )
                    },
                );
            }
        });
    }

    // The map-wide actions apply whether or not the spyglass bounds the
    // view, so they stand outside it; the two buttons are debug escapes.
    ui.add_space(FIELD_GAP);
    // How often the map goes back for what it already holds. Out here
    // rather than under the spyglass because it is not the spyglass's:
    // `bodies::fetch` asks the inside of a system on it, `filter::mark`
    // re-cuts the time filter on it, and `crate::map::index::refresh` picks up a
    // republished index on it. Not one of them is about the reach.
    ui.horizontal(|ui| {
        edited(&mut settings.poll, |x| &mut x.0, |wait| poll_value(ui, wait))
    });

    // What belongs to both views, which is what this section is for and
    // what it is named for. The galaxy drawn as a map or as a sky, and a
    // system seen from inside it, are two views with their own sections
    // below; a switch that governs both filed under either would read as
    // turning off only that half of it.
    //
    // The labels are that: a name is drawn over a system out among the
    // stars and over a body within one, and one key turns both off (see
    // [`crate::map::keys`]). They stood in the two view sections, which is
    // where a reader who wanted the names off had to find them twice. The
    // ruling is the same argument — one ruled plane carries the map from
    // light years down to light seconds.
    heading(ui, "General", true);
    // Named apart, where the two sections named both of them "Show
    // Labels" and left the heading over each to say which was meant.
    // Together they have to say it themselves.
    edited(
        &mut settings.show_names,
        |x| &mut x.0,
        |on| check(ui, on, "System Names", "Show system names on the map"),
    );
    if settings.show_names.0 {
        // Indented under what turns them on, since neither means anything
        // without it. The rule egui draws down the side of an indent says
        // as much, and says it without a heading standing over nothing
        // whenever the box is unchecked.
        ui.indent("names", |ui| {
            // The reach controls answer a map-view question — how far about
            // the center to name — and say nothing in the realistic view,
            // where a star earns its name by being bright enough to draw
            // rather than by standing near the center. See
            // [`crate::map::labels::worth_placing`].
            if *settings.view == View::Map {
                edited(
                    &mut settings.name_radius,
                    |x| &mut x.follow_spyglass,
                    |on| {
                        check(
                            ui,
                            on,
                            "Names Follow Spyglass",
                            "Name systems out to the spyglass radius",
                        )
                    },
                );
                if !settings.name_radius.follow_spyglass {
                    // A name can only be drawn for a system that is drawn,
                    // and the spyglass decides that. One that is not
                    // clearing draws everything loaded, and then names may
                    // be asked for beyond its reach.
                    let ceiling = if settings.spyglass.clear {
                        settings.spyglass.radius
                    } else {
                        Spyglass::CEILING
                    };
                    titled(
                        ui,
                        "Name Radius (Ly)",
                        "How far from the center to show names",
                    );
                    edited(
                        &mut settings.name_radius,
                        |x| &mut x.radius,
                        |radius| radius_slider(ui, radius, ceiling),
                    );
                }
            } else {
                // The realistic view has no reach to hold names to, so it
                // holds them to a brightness instead: named brightest
                // first, down to this limiting magnitude. Turning it down
                // names fewer of them, the way Name Radius names fewer in
                // the map view. A star past the exposure's floor is not
                // drawn and so cannot be named whatever this says.
                titled(
                    ui,
                    "Name Limit (mag)",
                    "Only name stars brighter than this",
                );
                let mut mag = settings.name_limit.0;
                fill_width(ui, VALUE_WIDTH);
                let slider = ui
                    .horizontal(|ui| {
                        let rail = ui.add(
                            egui::Slider::new(&mut mag, -2.0..=12.0)
                                .step_by(0.5)
                                .show_value(false),
                        );
                        let typed = value_box(
                            ui,
                            egui::DragValue::new(&mut mag)
                                .range(-2.0..=12.0)
                                .speed(0.1)
                                .suffix(" mag"),
                        );
                        rail | typed
                    })
                    .inner;
                // Only when it lands somewhere new, as the exposure slider
                // is, so a still slider does not mark the resource changed.
                if slider.changed() && settings.name_limit.0 != mag {
                    settings.name_limit.0 = mag;
                }
            }
        });
    }

    edited(
        &mut settings.show_body_names,
        |x| &mut x.0,
        |on| check(ui, on, "Body Names", "Show body names inside a system"),
    );
    // The one reading among the switches, and here because it is drawn
    // over both views as the names and the ruling are: what the map is
    // standing at is as true inside a system as out among the stars.
    //
    // Turning it off is the map at the present, which the hint says
    // because the reading is the only place a run-on is shown and the
    // only way back from one.
    edited(
        &mut settings.show_clock,
        |x| &mut x.0,
        |on| {
            check(
                ui,
                on,
                "Clock",
                "Show the date, and draw the map at the present",
            )
        },
    );
    ui.add_space(FIELD_GAP);
    edited(
        &mut settings.show_grid,
        |x| &mut x.0,
        |on| check(ui, on, "Grid", "Show the measuring grid"),
    );
    if settings.show_grid.0 {
        // Indented under what turns them on, the same as the names are,
        // since a unit for a ruler that is not drawn is a choice about
        // nothing. Left to the map by default, which turns the ruler over
        // as it descends into a system; pinned either way for reading a
        // system's distances in light years or a neighbourhood's in light
        // seconds.
        ui.indent("said", |ui| {
            edited(
                &mut settings.show_middle,
                |x| &mut x.0,
                |on| {
                    check(
                        ui,
                        on,
                        "Show Center Position",
                        "Show coordinates of the view center",
                    )
                },
            );
            edited(
                &mut settings.show_picked,
                |x| &mut x.0,
                |on| {
                    check(
                        ui,
                        on,
                        "Show Selected Positions",
                        "Show coordinates of selected systems",
                    )
                },
            );
            ui.add_space(FIELD_GAP);
            // How loudly the whole ruling is drawn, lines and numbers
            // together. Past a hundred for a ruler that has to be read off
            // a bright field, under it for one that should stay out of the
            // way of a busy sky.
            titled(ui, "Brightness (%)", "How bright the grid is drawn");
            let mut bright = settings.bright.0 * 100.;
            fill_width(ui, VALUE_WIDTH);
            let slider = ui
                .horizontal(|ui| {
                    let rail = ui.add(
                        egui::Slider::new(&mut bright, 0.0..=100.)
                            .step_by(5.)
                            .show_value(false),
                    );
                    let typed = value_box(
                        ui,
                        egui::DragValue::new(&mut bright)
                            .range(0.0..=100.)
                            .suffix("%"),
                    );
                    rail | typed
                })
                .inner;
            // Only on a change. Written every frame it would mark the
            // resource changed every frame, and the planes are rebuilt
            // from it.
            if slider.changed() {
                settings.bright.0 = bright / 100.;
            }
            ui.add_space(FIELD_GAP);
            titled(ui, "Units", "What the grid is measured in");
            edited(
                &mut settings.unit,
                |x| x,
                |unit| {
                    choose(
                        ui,
                        unit,
                        RulerUnit::Automatic,
                        "Automatic",
                        "Light years in space, light seconds in a system",
                    );
                    choose(
                        ui,
                        unit,
                        RulerUnit::LightYears,
                        "Light Years",
                        "Always light years",
                    );
                    choose(
                        ui,
                        unit,
                        RulerUnit::LightSeconds,
                        "Light Seconds",
                        "Always light seconds",
                    );
                },
            );
        });
    }

    // Which of the two ways the sky itself is drawn, and what each of
    // them offers. What is named over it went up to General, a name being
    // drawn either way.
    heading(ui, "Galaxy View", true);
    edited(
        &mut settings.view,
        |x| x,
        |view| {
            choose(
                ui,
                view,
                View::Map,
                "Map",
                "Flat colored dots, one per system",
            );
            choose(
                ui,
                view,
                View::Realistic,
                "Realistic",
                "Stars at their real color and brightness",
            );
        },
    );
    if *settings.view == View::Map {
        ui.add_space(FIELD_GAP);
        titled(ui, "Color By", "What a system's color means");
        edited(
            &mut settings.color_by,
            |x| x,
            |color_by| {
                choose(
                    ui,
                    color_by,
                    ColorBy::Allegiance,
                    "Allegiance",
                    "Color by controlling power",
                );
                choose(
                    ui,
                    color_by,
                    ColorBy::Government,
                    "Government",
                    "Color by government type",
                );
                choose(
                    ui,
                    color_by,
                    ColorBy::Security,
                    "Security",
                    "Color by security level",
                );
            },
        );
        ui.add_space(FIELD_GAP);
        edited(
            &mut settings.population_scale,
            |x| &mut x.0,
            |on| {
                check(
                    ui,
                    on,
                    "Scale w/ Population",
                    "Size systems by population; hide empty ones",
                )
            },
        );
        ui.add_space(FIELD_GAP);
        // How many stops the field behind the marks is lifted by. The
        // map is a political instrument at one setting and a picture of
        // where anybody has been at another, and which of those a reader
        // wants is theirs to say; the roll-off on the packed end goes on
        // holding the core down either way. The marks are not on this
        // dial, a drawn system being an object at a set brightness.
        //
        // **Three stops at the top, and the rail spends its length on
        // what is below.** The field's own level is settled against
        // the reach now (`glow::TILT`), so the dial is no longer
        // carrying three stops of that on top of a reading — and read
        // off the map, three stops over the rest is as bright as the
        // galaxy is ever wanted. A rail that ran to eight spent more
        // than half its travel past anything usable, which is a dial
        // that cannot be set finely where it is actually set.
        titled(
            ui,
            "Field Exposure (EV)",
            "How brightly the galaxy behind the marks is drawn",
        );
        let mut field_ev = settings.field_exposure.0;
        fill_width(ui, VALUE_WIDTH);
        let slider = ui
            .horizontal(|ui| {
                let rail = ui.add(
                    egui::Slider::new(&mut field_ev, -9.0..=3.0)
                        .step_by(0.25)
                        .show_value(false),
                );
                let typed = value_box(
                    ui,
                    egui::DragValue::new(&mut field_ev)
                        .range(-9.0..=3.0)
                        .speed(0.1)
                        .suffix(" EV"),
                );
                rail | typed
            })
            .inner;
        // Only when it lands somewhere new, so a still slider does not
        // mark the resource changed every frame.
        if slider.changed() && settings.field_exposure.0 != field_ev {
            settings.field_exposure.0 = field_ev;
        }
    }
    if *settings.view == View::Realistic {
        ui.add_space(FIELD_GAP);
        // The point-spread profile the stars wear: a Moffat with its wings
        // or a tighter Gaussian. Read into a local and written back only on
        // a change, so drawing the radios does not mark the resource changed
        // every frame and cut the texture again; see
        // [`crate::map::galaxy::spawn::reprofile`].
        titled(ui, "Point spread", "How a star's light blurs");
        let mut profile = settings.star_profile.0;
        for choice in ProfileKind::ALL {
            choose(
                ui,
                &mut profile,
                choice,
                choice.name(),
                spread_hint(choice),
            );
        }
        if profile != settings.star_profile.0 {
            settings.star_profile.0 = profile;
        }
        ui.add_space(FIELD_GAP);
        // How many stops the star field is lifted to the display. From a
        // sky bright enough for only the most luminous stars, up through
        // the dark-adapted field at zero to several stops past it, where
        // the faint sky fills in.
        titled(ui, "Exposure (EV)", "How brightly the stars are exposed");
        let mut ev = settings.star_exposure.0;
        fill_width(ui, VALUE_WIDTH);
        let slider = ui
            .horizontal(|ui| {
                let rail = ui.add(
                    egui::Slider::new(&mut ev, -12.0..=8.0)
                        .step_by(0.5)
                        .show_value(false),
                );
                let typed = value_box(
                    ui,
                    egui::DragValue::new(&mut ev)
                        .range(-12.0..=8.0)
                        .speed(0.1)
                        .suffix(" EV"),
                );
                rail | typed
            })
            .inner;
        // Only when it lands somewhere new, so a slider reporting the same
        // value frame after frame does not mark `StarExposure` changed and
        // trip every reader of it needlessly.
        if slider.changed() && settings.star_exposure.0 != ev {
            settings.star_exposure.0 = ev;
        }
    }

    // What is drawn once the camera is inside a system, rather than what
    // the galaxy is drawn as. Its own section for that reason, and not
    // under the view above it: which of the two ways the sky is drawn says
    // nothing about what a system looks like from within. The body names
    // went up to General with the system names, one key turning both off
    // and a reader wanting them off having had to find them twice.
    heading(ui, "System View", true);
    edited(
        &mut settings.show_orbits,
        |x| &mut x.0,
        |on| check(ui, on, "Orbit Lines", "Show the orbit each body follows"),
    );

    // How the filters answer, rather than which they are: the filters
    // themselves are asked for in the bar, and this is the one thing
    // about them that is set once and left alone.
    heading(ui, "Filters", true);
    titled(
        ui,
        "Filtered Opacity (%)",
        "How faintly unmatched systems are drawn",
    );
    let mut showing = dim.0 * 100.;
    fill_width(ui, VALUE_WIDTH);
    let slider = ui
        .horizontal(|ui| {
            let rail = ui.add(
                egui::Slider::new(&mut showing, 0.0..=100.)
                    .step_by(5.)
                    .show_value(false),
            );
            let typed = value_box(
                ui,
                egui::DragValue::new(&mut showing)
                    .range(0.0..=100.)
                    .suffix("%"),
            );
            rail | typed
        })
        .inner;
    // Only when it lands on a value the resource does not already hold. The
    // step and the f32 round-trip can have the slider report a change frame
    // after frame at a value it is already at — 0.30 is not exactly
    // representable, so `dim.0 * 100` snapped back to `/ 100` never settles
    // — and writing that every frame marks the resource changed every
    // frame, which repaints every dimmed star and re-marks the whole sky
    // without end.
    if slider.changed() {
        let set = showing / 100.;
        if dim.0 != set {
            dim.0 = set;
        }
    }
    // Which is a filter in the plainer sense: this kind of system and
    // none of the rest. At zero the excluded are not dimmed but dropped —
    // never loaded, and evicted if already on the map — so the sign is that
    // they are not there rather than that they are faint.
    if dim.0 == 0. {
        ui.label(egui::RichText::new("Not loaded").weak());
    }
}

/// Draw `widget` over a copy of what `at` picks out of `resource`, and write
/// it back only where the widget changed it
///
/// A widget takes a `&mut` to what it sets, and a `ResMut` handed out as one
/// reads as written whether or not anything was, so a pane that passed its
/// settings straight in would mark every one of them changed every frame it
/// stood open. What reads the mark does the work again:
/// [`crate::map::galaxy::walk`] reconciles the whole sky on a [`View`] or a
/// [`ScalePopulation`] said to have moved, and the blobs are rebuilt on a
/// [`ColorBy`]. The copy is what the widget works on, and the resource is
/// touched only when the copy comes back different.
///
/// The sliders that settle a value of their own first — the exposures, the
/// grid's brightness, the dimming — and [`StarProfile`] already write back
/// only on a change, and are left as they are.
fn edited<R, T, W>(
    resource: &mut R,
    at: impl Fn(&mut R::Inner) -> &mut T,
    widget: impl FnOnce(&mut T) -> W,
) -> W
where
    R: DetectChangesMut,
    T: Clone + PartialEq,
{
    let mut value = at(resource.bypass_change_detection()).clone();
    let drawn = widget(&mut value);
    let held = at(resource.bypass_change_detection());
    if *held != value {
        *held = value;
        resource.set_changed();
    }
    drawn
}

/// Slide the settings pane in from the left, and draw `contents` in it
///
/// Answers how far its right edge has reached, which is where the gear
/// stands. Zero while the pane is shut, so the gear sits in the corner and
/// rides the pane's edge as it comes out.
///
/// An [`egui::Area`] rather than a panel, so that the pane travels in from
/// off the viewport rather than growing in place, and because every top-level
/// `Panel::show` is deprecated with nothing at the top level to replace it.
pub(super) fn settings_pane(
    ctx: &Context,
    open: bool,
    contents: impl FnOnce(&mut Ui),
) -> f32 {
    // Asked for every frame, shown or not, since this is what advances the
    // slide and answers when it has finished.
    let out = ctx.animate_bool(egui::Id::new("settings-pane"), open);
    if out == 0. {
        return 0.;
    }

    let height = ctx.content_rect().height();
    let style = ctx.global_style();
    // Square, since three of its four sides are off the viewport, and edged
    // so that the one that is not reads against the map behind it.
    let frame = egui::Frame::side_top_panel(&style)
        .stroke(style.visuals.window_stroke())
        .shadow(style.visuals.window_shadow);
    let margins = frame.total_margin().sum();

    zone("settings-pane")
        .fixed_pos(egui::pos2((out - 1.) * PANE_WIDTH, 0.))
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(PANE_WIDTH - margins.x);
                ui.set_height(height - margins.y);
                // The bar stands beside what it scrolls rather than over it,
                // so that the width asked for below is the width there is.
                // Floated, it would be drawn across the right hand end of
                // every slider in the pane.
                ui.spacing_mut().scroll.floating = false;
                egui::ScrollArea::vertical().show(ui, contents);
            });
        })
        .response
        .rect
        .right()
}

/// The handle on the settings pane, alone in the corner it opens from
///
/// Bare, so that what stands in the corner is a gear rather than a box with a
/// gear in it. It rides the pane's edge at `left`, since a handle the pane
/// slides over is a handle the user cannot reach.
///
/// `middle` is where the bar's search box sits, and the gear is hung about it
/// rather than dropped from the top of the viewport as the bar is. The two
/// stand side by side, so what lines them up is the field the user is looking
/// at rather than the top edge of a box the field is padded inside.
///
/// It is given [`GEAR_ROOM`] across, that being what the bar leaves for it.
/// Measured instead, the room would not be known until the gear had been
/// drawn, and the gear cannot be drawn until the bar has said where its search
/// box is.
pub(super) fn gear(ctx: &Context, left: f32, middle: f32, open: &mut bool) {
    let style = ctx.global_style();
    let clicked = zone("settings-gear")
        .pivot(egui::Align2::LEFT_CENTER)
        .fixed_pos(egui::pos2(left + MARGIN, middle))
        .show(ctx, |ui| {
            let mut gear = egui::RichText::new("⚙").size(GEAR_SIZE);
            if *open {
                gear = gear.color(style.visuals.strong_text_color());
            }
            ui.add_sized(
                egui::vec2(GEAR_ROOM, 0.),
                egui::Button::new(gear).frame(false),
            )
            .clicked()
        })
        .inner;

    if clicked {
        *open = !*open;
    }
}

/// What each point-spread profile does to a star, in a line
///
/// Its own function rather than a method on the kind, the kind belonging to
/// [`galos_photometry`] and this being what the pane says about it rather than
/// what it is.
fn spread_hint(kind: ProfileKind) -> &'static str {
    match kind {
        ProfileKind::Moffat => "Soft halo, like a telescope",
        ProfileKind::Gaussian => "Tight dot, no halo",
    }
}

/// One of several, and what choosing it does. See [`check`].
fn choose<T: PartialEq>(
    ui: &mut Ui,
    held: &mut T,
    value: T,
    said: &str,
    hint: &str,
) -> Response {
    ui.radio_value(held, value, said).on_hover_text(hint)
}

/// The name over a slider or a value, and what it sets. See [`check`].
fn titled(ui: &mut Ui, said: &str, hint: &str) -> Response {
    ui.label(said).on_hover_text(hint)
}

/// Open a section, in the form or in the settings pane
///
/// The rule is the break between one section and the next, and needs no
/// run-up of its own: the row above it keeps as much room under itself as it
/// keeps over, and a section gap on top of that would sit the row nearer the
/// input than the section and read as belonging to neither.
///
/// `ruled` is how the first section of the pane goes without one. The top of
/// the pane is already an edge, and a rule drawn against it reads as a
/// section with nothing in it. Every section of the form is ruled, having the
/// input and the selection's row above it.
fn heading(ui: &mut Ui, name: &str, ruled: bool) {
    if ruled {
        ui.separator();
    }
    ui.label(egui::RichText::new(name).strong());
    ui.add_space(FIELD_GAP);
}

fn poll_value(ui: &mut Ui, opt: &mut Option<f64>) {
    let mut enabled = opt.is_some();
    if check(ui, &mut enabled, "Poll", "Fetch fresh data periodically")
        .changed()
    {
        if enabled {
            // Turned back on at what it opened at, the wait it was left at
            // having gone when it was turned off.
            *opt = Some(10.);
        } else {
            *opt = None
        }
    }

    // The unit stands in the box with the number, so the row is the name of
    // the thing and the value of it and nothing between them.
    if let Some(val) = opt {
        ui.add(
            egui::DragValue::new(val).range(0.0..=60.).speed(0.01).suffix(" s"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ui::testing::{SLACK, clicking, spoken_at};

    /// A world holding everything the settings pane sets, over `view`
    ///
    /// Every switch that folds more of the pane out is on, so that every
    /// widget in it is drawn, and the trackers are cleared: whatever reads
    /// as changed afterwards was marked by what the pane did.
    fn pane_world(view: View) -> World {
        let mut world = World::new();
        world.insert_resource(view);
        world.insert_resource(Spyglass {
            radius: 50.,
            clear: true,
            lock_camera: false,
            follow_camera: false,
        });
        world.insert_resource(ColorBy::Allegiance);
        world.insert_resource(ScalePopulation(false));
        world.insert_resource(StarExposure::default());
        world.insert_resource(FieldExposure(0.));
        world.insert_resource(StarProfile::default());
        world.insert_resource(ShowNames(true));
        world.insert_resource(Poll(Some(10.)));
        world.insert_resource(NameRadius {
            follow_spyglass: false,
            radius: 20.,
        });
        world.insert_resource(NameLimit(6.));
        world.insert_resource(ShowOrbits(true));
        world.insert_resource(Clock::default());
        world.insert_resource(ClockControl::default());
        world.insert_resource(ShowClock::default());
        world.insert_resource(ShowBodyNames(true));
        world.insert_resource(ShowGrid(true));
        world.insert_resource(RulerUnit::default());
        world.insert_resource(ShowMiddle(false));
        world.insert_resource(ShowPicked(false));
        world.insert_resource(Bright(1.));
        world.insert_resource(DimTo(0.3));
        world.clear_trackers();
        world
    }

    /// Draw the settings pane over `world` once for each of `inputs`
    ///
    /// Answers with what the last pass drew.
    fn pane_drawn(
        world: &mut World,
        inputs: Vec<egui::RawInput>,
    ) -> egui::FullOutput {
        use bevy::ecs::system::RunSystemOnce;

        let ctx = crate::testing::context();
        world
            .run_system_once(
                move |mut settings: Settings, mut dim: ResMut<DimTo>| {
                    let mut last = None;
                    for input in inputs.clone() {
                        last = Some(ctx.run_ui(input, |ui| {
                            settings_body(ui, &mut settings, &mut dim);
                        }));
                    }
                    last.expect("the pane was drawn at least once")
                },
            )
            .expect("the pane's resources are all there")
    }

    /// Whether `R` has been marked changed since the world last cleared its
    /// trackers
    fn touched<R: Resource>(world: &World) -> bool {
        world.is_resource_changed::<R>()
    }

    /// A pane standing open and left alone marks nothing it sets as changed
    ///
    /// Every widget in it is handed a `&mut`, and a `ResMut` reads as written
    /// the moment one is taken. Handed over straight, an open pane marked
    /// every setting changed every frame, and [`crate::map::galaxy::walk`]
    /// reconciled the whole sky again on a [`View`] nobody had touched.
    ///
    /// Drawn over both views, each having a section of its own.
    #[test]
    fn an_untouched_pane_leaves_its_settings_unchanged() {
        for view in [View::Map, View::Realistic] {
            let mut world = pane_world(view);
            // Fonts are built on the first pass; the second is the pane as it
            // stands frame after frame.
            let _ = pane_drawn(
                &mut world,
                vec![egui::RawInput::default(), egui::RawInput::default()],
            );

            let changed: Vec<&str> = [
                ("View", touched::<View>(&world)),
                ("ColorBy", touched::<ColorBy>(&world)),
                ("ScalePopulation", touched::<ScalePopulation>(&world)),
                ("Spyglass", touched::<Spyglass>(&world)),
                ("StarExposure", touched::<StarExposure>(&world)),
                ("FieldExposure", touched::<FieldExposure>(&world)),
                ("StarProfile", touched::<StarProfile>(&world)),
                ("ShowNames", touched::<ShowNames>(&world)),
                ("Poll", touched::<Poll>(&world)),
                ("NameRadius", touched::<NameRadius>(&world)),
                ("NameLimit", touched::<NameLimit>(&world)),
                ("ShowOrbits", touched::<ShowOrbits>(&world)),
                ("ShowClock", touched::<ShowClock>(&world)),
                ("ShowBodyNames", touched::<ShowBodyNames>(&world)),
                ("ShowGrid", touched::<ShowGrid>(&world)),
                ("RulerUnit", touched::<RulerUnit>(&world)),
                ("ShowMiddle", touched::<ShowMiddle>(&world)),
                ("ShowPicked", touched::<ShowPicked>(&world)),
                ("Bright", touched::<Bright>(&world)),
                ("DimTo", touched::<DimTo>(&world)),
            ]
            .into_iter()
            .filter_map(|(name, marked)| marked.then_some(name))
            .collect();
            assert!(
                changed.is_empty(),
                "an untouched pane over {view:?} marked {changed:?} changed",
            );
        }
    }

    /// And a switch clicked in it is written, and marked
    ///
    /// The other half of the guard: a pane that wrote back nothing would be a
    /// picture of the settings rather than a way to set them.
    #[test]
    fn a_switch_clicked_in_the_pane_is_written() {
        let mut world = pane_world(View::Map);
        let output = pane_drawn(
            &mut world,
            vec![egui::RawInput::default(), egui::RawInput::default()],
        );
        let switch =
            spoken_at(&output, "Orbit Lines").expect("the switch is drawn");

        let _ = pane_drawn(
            &mut world,
            vec![
                egui::RawInput::default(),
                egui::RawInput::default(),
                clicking(switch.center()),
            ],
        );

        assert!(!world.resource::<ShowOrbits>().0, "the click went unwritten");
        assert!(touched::<ShowOrbits>(&world));
        assert!(
            !touched::<View>(&world),
            "a click on one switch marked the view"
        );
    }

    /// The chrome, drawn with the pointer at `at`, once it stands still
    ///
    /// Several frames, because the pane slides in on an animation and is not
    /// drawn at all while the slide stands at nothing: what is wanted is the
    /// frame after it has finished, which is the pane as the user meets it.
    fn chromed(at: egui::Pos2) -> (egui::Context, egui::FullOutput) {
        let ctx = crate::testing::context();
        let input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(at)],
            predicted_dt: 1. / 60.,
            ..Default::default()
        };
        let mut output = None;
        for _ in 0..60 {
            output = Some(ctx.run_ui(input.clone(), |ui| {
                let ctx = ui.ctx().clone();
                // A ring, as `selection::ring` paints one, into the layer the
                // map puts its annotations in.
                ctx.layer_painter(crate::map::screen::annotations_layer())
                    .circle_stroke(
                        egui::pos2(PANE_WIDTH * 0.5, 300.),
                        12.,
                        egui::Stroke::new(2_f32, egui::Color32::YELLOW),
                    );
                settings_pane(&ctx, true, |ui| {
                    ui.label("Spyglass");
                });
            }));
        }

        (ctx, output.expect("a frame was drawn"))
    }

    /// A wheel turned over the chrome is not a wheel turned at the map
    ///
    /// Reported: scrolling the settings pane zoomed the map behind it. The
    /// camera asks [`PointerOverUi`], which is `is_pointer_over_egui`, and
    /// that answers false for anything in `Order::Background` inside the root
    /// ui's available rect — which the whole of the chrome was. Nothing about
    /// the guard was wrong; egui did not count the pane as its own.
    #[test]
    fn the_pointer_over_the_chrome_is_egui_s() {
        let (inside, _) = chromed(egui::pos2(20., 300.));
        assert!(
            inside.is_pointer_over_egui(),
            "a pointer over the pane was the map's"
        );

        let (outside, _) = chromed(egui::pos2(PANE_WIDTH + 400., 300.));
        assert!(
            !outside.is_pointer_over_egui(),
            "a pointer out on the map was the chrome's"
        );
    }

    /// And what the map annotates is painted under the chrome, not over it
    ///
    /// The other half of the same report: a selection ring and a name plate
    /// were drawn over the pane. The annotations are one painter list rather
    /// than an area, and a layer that is not an area is drained after every
    /// area of its own order, so sharing `Background` with the chrome put
    /// them on top of it however the two were ordered against each other.
    #[test]
    fn the_chrome_is_painted_over_the_annotations() {
        let (_, output) = chromed(egui::pos2(20., 300.));
        let at = |what: fn(&egui::Shape) -> bool| {
            output.shapes.iter().position(|clipped| what(&clipped.shape))
        };
        let ring = at(|shape| matches!(shape, egui::Shape::Circle(_)))
            .expect("the ring was painted");
        let pane = at(|shape| matches!(shape, egui::Shape::Rect(_)))
            .expect("the pane was painted");

        assert!(ring < pane, "the ring was painted over the pane");
    }

    /// The gear hangs about the height it is given
    ///
    /// Which is where the bar's search box came out, so that the handle and
    /// the field beside it read as one row. Dropped from the top of the
    /// viewport as the bar is, it would sit level with the top edge of a box
    /// the field is padded inside rather than with the field.
    ///
    /// Twice round, since an area is placed about a pivot from the size it
    /// came out last time and has no size at all the first time it is drawn.
    #[test]
    fn the_gear_hangs_about_the_height_it_is_given() {
        let ctx = Context::default();
        let middle = 40.;
        let mut open = false;

        for _ in 0..2 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                gear(ui.ctx(), 0., middle, &mut open);
            });
        }
        let at = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("settings-gear")))
            .expect("a gear was drawn");

        // Within half a pixel: the gear stands an odd number of them tall,
        // and egui rounds where an area is put onto the pixel grid.
        let off = (at.center().y - middle).abs();
        assert!(off <= 0.5, "{off} off the {middle} it was given");
    }

    /// A slider row drawn in the pane, as it comes out
    ///
    /// The row it filled, the room it was given, and how wide the box holding
    /// the value came out. Drawn the way the pane draws one rather than
    /// measured off the style, since what is asked for and what is taken are
    /// different questions and only the second is on screen.
    ///
    /// The pane slides in rather than appearing, and `animate_bool` wants time
    /// to pass before it is all the way out, so nothing is drawn inside it on
    /// the first frame. Hence the run of them, with the clock moving.
    struct Row {
        used: f32,
        room: f32,
    }

    /// Draw a real radius slider in a pane `width` wide, indented or not
    fn slider_row(width: f32, indented: bool) -> Row {
        let ctx = crate::testing::context();
        let mut row = Row { used: 0., room: 0. };
        let mut radius = 10_f32;
        for frame in 0..10 {
            let input = egui::RawInput {
                time: Some(frame as f64 * 0.1),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 600.),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                settings_pane(ui.ctx(), true, |ui| {
                    let mut draw = |ui: &mut Ui| {
                        row.room = ui.available_width();
                        row.used =
                            radius_slider(ui, &mut radius, Spyglass::CEILING)
                                .rect
                                .width();
                    };
                    if indented {
                        ui.indent("test", draw);
                    } else {
                        draw(ui);
                    }
                });
            });
        }
        row
    }

    /// What a radius comes out at, having been offered up to `ceiling`
    fn drawn_radius(start: f32, ceiling: f32) -> f32 {
        let ctx = crate::testing::context();
        let mut radius = start;
        for frame in 0..10 {
            let input = egui::RawInput {
                time: Some(frame as f64 * 0.1),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 600.),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                settings_pane(ui.ctx(), true, |ui| {
                    radius_slider(ui, &mut radius, ceiling);
                });
            });
        }
        radius
    }

    /// A radius past the galaxy's edge is brought inside it
    ///
    /// That bound does not move, so a radius past it is not a setting the map
    /// cannot honor yet but one it can never honor, and correcting it loses
    /// nothing that could come back.
    #[test]
    fn a_radius_is_held_within_the_galaxy() {
        assert_eq!(drawn_radius(5e6, 5e6), Spyglass::CEILING);
    }

    /// A radius under the rail's least is kept, not written up to it
    ///
    /// The reach follows the camera all the way in
    /// ([`crate::map::galaxy::reach_with_camera`]), so a reach drawn in under the
    /// rail is a real setting. Written back, this pane being open would hold
    /// the sky within a thousandth of a light year open every frame and put
    /// the neighbours of a system flown into back on the map.
    #[test]
    fn a_radius_under_the_rail_is_kept() {
        assert_eq!(drawn_radius(1e-6, 100.), 1e-6);
    }

    /// A ceiling that comes down does not take the setting with it
    ///
    /// What is asked for and what can be drawn are two questions. The
    /// spyglass answers the second and moves as the map is flown, so letting
    /// it write the first loses the asking the moment it drops underneath,
    /// with nothing said and no way back but to ask again.
    #[test]
    fn a_radius_over_the_ceiling_is_kept() {
        assert_eq!(drawn_radius(5e4, 100.), 5e4);
    }

    /// How much of the value one pixel of a `rail` pixels wide is worth
    ///
    /// The rail is logarithmic over the whole range, so a pixel is a fixed
    /// multiple of whatever the value is rather than a fixed distance. This
    /// is that multiple, less the one, so it reads as the fraction the drag
    /// speed is also given as.
    fn rail_precision(rail: f32) -> f32 {
        let decades = (Spyglass::CEILING / Spyglass::FLOOR).log10();
        10_f32.powf(decades / rail) - 1.
    }

    /// The box beside a radius is finer than the rail
    ///
    /// Which is the whole reason for having both. The rail crosses four
    /// decades in the width of the pane and so reaches anywhere and settles
    /// on nothing; the box is what settles. A box no finer than the rail is
    /// a second way to do the same coarse thing.
    #[test]
    fn the_radius_box_is_finer_than_its_rail() {
        // What the row came to, less the box at its end and the gap before it.
        let gap = egui::style::Spacing::default().item_spacing.x;
        let rail =
            rail_precision(slider_row(1280., false).used - VALUE_WIDTH - gap);

        assert!(
            RADIUS_DRAG * 5. < rail,
            "a drag worth {RADIUS_DRAG} of the value against a rail worth \
             {rail} of it is no finer to speak of"
        );
    }

    /// A ceiling at the floor is still a radius
    ///
    /// Names reach no further than the spyglass, so a spyglass wound all the
    /// way in leaves them a range with no room in it at all. A logarithmic
    /// rail divides by the span it is given.
    #[test]
    fn a_range_with_no_room_in_it_still_draws() {
        assert_eq!(
            drawn_radius(Spyglass::FLOOR, Spyglass::FLOOR),
            Spyglass::FLOOR
        );
    }

    /// A slider row fills the pane it is drawn in
    ///
    /// Egui sizes a rail from the style rather than from the room it is given,
    /// and sizes the box beside it to the number in it, so left alone a row
    /// is an island of the same hundred pixels and a ragged box, ending
    /// wherever that happens to leave it.
    #[test]
    fn a_slider_row_fills_the_pane() {
        for width in [1280., 400.] {
            let row = slider_row(width, false);

            assert!(
                (row.used - row.room).abs() < SLACK,
                "in a pane {width} wide a row of {} filled {} of it",
                row.used,
                row.room
            );
        }
    }

    /// And fills an indent, which is narrower than the pane
    ///
    /// The name radius hangs under the checkbox that turns names on, so it is
    /// drawn a step in from the edge. Sized once for the pane it would run out
    /// past the end of its own line by exactly that step.
    #[test]
    fn a_slider_row_fills_an_indent() {
        let indented = slider_row(1280., true);
        let plain = slider_row(1280., false);

        assert!(
            indented.room < plain.room,
            "an indent of {} is no narrower than the {} around it",
            indented.room,
            plain.room
        );
        assert!(
            (indented.used - indented.room).abs() < SLACK,
            "a row of {} filled {} of the indent",
            indented.used,
            indented.room
        );
    }
}
