//! How a route is weighed and planned, and what a plot under way says about
//! itself

use crate::map::route::frontier::Frontiers;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::Ui;
use galos_route::graph::{Crossing, Drive, Routing, Tuning, Weigh};
use galos_route::{graph, highway};

/// What a plot under way says about itself, and what to turn down
///
/// A route across the galaxy expands hundreds of thousands of systems over
/// several seconds, and a spinner says the map is working without saying
/// whether it is getting anywhere. The map draws the same progress out on
/// the sky; this is the number beside the button. Four readings, each asked
/// separately because each arrives at its own moment: the clock as the
/// button is pressed, the count once the graph is open and the first systems
/// are expanded, and how far there is to go once anything has been reached
/// at all. The wait comes first, that being what a wait is actually about —
/// and the one reading that says a search is alive while a gap between two
/// boost stars expands nothing anybody can see.
///
/// **One line about the plot, not one per leg.** A trip's legs are searched
/// at once, and each reading is an aggregate over them: the longest wait,
/// the expansions added up, the distances left added up. How many legs are
/// still being worked out is said where there is more than one, the other
/// three reading differently about four searches than about one.
///
/// And under it, where the wait has run long, the setting that would buy it
/// back — see [`quicker_by`].
pub(super) fn searching_says(
    ui: &mut Ui,
    searching: &Frontiers,
    how: &Routing,
    tune: &Tuning,
    drive: Drive,
) {
    let expanded = searching.expanded();
    let mut said = Vec::with_capacity(4);
    if let Some(took) = searching.asked_for() {
        said.push(crate::ui::text::waited(took));
    }
    let legs = searching.legs();
    if legs > 1 {
        said.push(format!("{legs} legs"));
    }
    if expanded > 0 {
        said.push(format!(
            "{} systems searched",
            crate::ui::text::thousands(expanded)
        ));
    }
    // What answers the question a wait asks: the map draws the chains
    // out on the sky, and this is how far they still have to go, over
    // every leg still looking.
    if let Some(left) = searching.left() {
        said.push(format!(
            "{} Ly to go",
            crate::ui::text::thousands(left.round() as u64)
        ));
    }
    if !said.is_empty() {
        ui.label(egui::RichText::new(said.join(", ")).weak());
    }
    // And, for a wait long enough to be worth doing something about,
    // which setting to turn down. In yellow rather than red: nothing
    // has gone wrong, and the route being looked for is the one that
    // was asked for. See [`quicker_by`].
    if let Some(quicker) = searching
        .asked_for()
        .and_then(|took| quicker_by(took, how, tune, drive))
    {
        ui.colored_label(egui::Color32::YELLOW, quicker);
    }
}

/// How long a plot may run before the form offers a way to make it quicker
///
/// Long enough that an ordinary plot never says anything — the guard's rows
/// are milliseconds to a second or two — and short enough that a reader who
/// is about to give up is told first. See [`quicker_by`].
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(8);

/// Which setting to turn down, for a plot that is taking too long
///
/// **The settings are left erring toward the better route, and this is what
/// pays for that.** Every one of them trades the wait for the answer, and
/// which of them is costing the wait is not something a reader can see: the
/// map knows, because it knows what was asked. So a plot that runs past
/// [`PATIENCE`] says which slider buys the time back, in the words the
/// control itself is labelled with.
///
/// Ranked by what each is measured to cost over `.index/full`, dearest
/// first, and only one is offered — a form listing three things to try is
/// a form that has not answered the question.
///
/// [`None`] before [`PATIENCE`], and for a plot whose settings are already
/// the cheap ones: there is then nothing honest to suggest, and a wait
/// with no advice attached is the truth about a galaxy this size.
fn quicker_by(
    took: std::time::Duration,
    how: &Routing,
    tune: &Tuning,
    drive: Drive,
) -> Option<&'static str> {
    if took < PATIENCE {
        return None;
    }
    // Gaps first: searching every one of them is the dearest setting here
    // by an order of magnitude. Measured, Sagittarius A* to Colonia at 50
    // ly and 80%: 73 stops in 16.92 s searched against 76 in 0.38 s
    // stepped.
    if tune.crossing == Crossing::Searched {
        return Some(
            "Crossing a gap: Stepped answers in a fraction of the time,              for a jump or two more",
        );
    }
    // Then the plan, which is only asked for at all where a cone can be
    // used: `optimal` is three to six percent of the jumps for nine to
    // twenty-nine times the wait.
    if drive != Drive::Unaided && tune.planning == 0 && tune.allowance.is_none()
    {
        return Some(
            "Plan: optimal if cheap keeps the exact chain where it lands              quickly and leans where it does not",
        );
    }
    // Then the route's own optimality, whose rail is the steepest thing in
    // the form: 100% is minutes where 80% is milliseconds on the same
    // corridor, and on the corridors measured every setting came back with
    // the same route.
    if !how.approximates() {
        return Some(
            "Within: a percent off proven is most of the wait, and often              the same route",
        );
    }
    if how.optimality() > 80 {
        return Some(
            "Within: 80% was eleven times quicker than the proven ask on              the corridors measured, with the same answer",
        );
    }
    None
}

/// The fold the planning settings stand in, and what it says of a proven
/// route
///
/// Offered whenever a supercharging drive is fitted rather than only where
/// it does something, because a fold that comes and goes reads as a control
/// the map is hiding — which is how it read. A route that is being proven
/// has nothing to plan, and it says so: the plan's own edges are lower
/// bounds that bound nothing about the route they lead to, so an answer
/// that has to be *true* leaves only the flat search. See
/// [`Routing::approximates`], which is where either rail's top stop lands.
pub(super) fn planned(ui: &mut Ui, how: Routing, tune: &mut Tuning) {
    let plans = how.approximates();
    // Said in the header, not inside the fold: the fold is shut by default,
    // and a reason nobody opens is no reason at all.
    let title = match plans {
        true => "Planning",
        false => "Planning (unused when proven)",
    };
    let why = match plans {
        true => "How the plan over the boost stars is worked out",
        false => {
            "An optimal route is searched system by system and never \
             planned, there being no bound to be had off a plan. Ask for \
             less than optimal to plan one."
        }
    };
    egui::CollapsingHeader::new(title)
        .id_salt("route-planning")
        .show(ui, |ui| {
            ui.add_enabled_ui(plans, |ui| planning(ui, tune));
        })
        .header_response
        .on_hover_text(why);
}

/// Where the shortest tie-break is remembered while the trade's handle is
/// away from the end that has it
///
/// The fuel end of the rail has no ordering the tick refines, so the ask
/// cannot live in [`Routing`] while the handle is down there — and a reader
/// who ticked it does not expect a trip down the rail and back to have
/// unticked it.
const SHORTEST: fn() -> egui::Id = || egui::Id::new("route-shortest");

/// What the route is weighed by: one rail, and the tie-break its top has
///
/// **Two controls where there were three, because two of the three were the
/// ends of one line.** A hop of the whole range prices a jump at a tankful,
/// which is the fewest-jumps ask arrived at from the other side — so the
/// dropdown's first entry and the hop rail's top stop were the same
/// question, and nothing on the form said so. Measured over `.index/full`,
/// Sol to Col 285 Sector ZQ-K C9-12 at a hundred light years of range, all
/// of them proven:
///
/// | asked | stops | fuel |
/// |---|---|---|
/// | fewest jumps | 3 | 1.774 |
/// | hop 100% | 3 | 1.740 |
/// | hop 75% | 4 | 1.157 |
/// | hop 50% | 5 | 0.876 |
/// | hop 25% | 8 | 0.508 |
/// | hop 0% | 36 | 0.191 |
///
/// One line, monotone in both columns. It is a rail now, and its ends say
/// what they are rather than showing a percentage a reader has to convert.
///
/// **And the shortest ask is a tick under it rather than a mode beside
/// it**, because that is what it is: `Weigh::Shortest` breaks the tie
/// between chains of the *same jump count*, which is only an ordering at
/// the top of the rail. As a third mode it hid the rail whenever it was
/// chosen, which read as the trade being unavailable rather than as the
/// tick being a refinement of one end of it.
pub(super) fn trading(ui: &mut Ui, how: &mut Routing, jump: Option<f64>) {
    let (hop, expand) = traded(how.weigh);
    let mut asked = hop as f64;
    // In light years, off the range typed above. A percentage of a range is
    // arithmetic a reader should not have to do to find out whether the
    // route will split its jumps to eleven light years or to two — and the
    // ends are the whole travel of it, so they are said as the ends rather
    // than as 0% and 100%. Without a range on record there is nothing to
    // take a percentage of, and the rail says the percentage itself.
    let said = move |held: f64, _: std::ops::RangeInclusive<usize>| match (
        jump,
        held.round() as u32,
    ) {
        (Some(range), hop) => format!("{:.1} Ly", range * hop as f64 / 100.),
        (None, hop) => format!("{hop}%"),
    };
    let rail = ui
        .horizontal(|ui| {
            // The ends name the routes they are, which is what a reader
            // came for, and the figure between them is the hop that gets
            // there. No name of its own: "shortest hop" said what the
            // number is where the ends say what it is *for*, and the two
            // together read as three labels on one control.
            ui.label(egui::RichText::new("least fuel").weak());
            let rail = ui.add(
                egui::Slider::new(&mut asked, 0.0..=100.0)
                    .step_by(5.)
                    .custom_formatter(said)
                    .show_value(true),
            );
            ui.label(egui::RichText::new("fewest jumps").weak());
            rail
        })
        .inner;
    // Whether the tie-break was asked for, kept across a trip down the
    // rail and back: the fuel end has no such ordering, so the ask cannot
    // be held in `how` while the handle is down there.
    let ties =
        ui.data_mut(|data| data.get_temp::<bool>(SHORTEST())).unwrap_or(false);
    if rail.changed() {
        how.weigh = match asked.round() as u32 {
            // The named ask at the top, which has a cheaper cost of its
            // own: a jump count is one integer where the trade is two.
            100 => match ties {
                true => Weigh::Shortest,
                false => Weigh::Jumps,
            },
            // **A proven ask stays proven at the fuel end**, where the
            // cap is the whole of the promise: the percent means nothing
            // there, so a reader who asked for optimal and then ran the
            // trade down to it would otherwise have the rail's remembered
            // cap quietly make the answer unprovable. See
            // [`Routing::approximates`].
            0 if how.over == 0 => Weigh::Fuel { hop: 0, expand: 0 },
            hop => Weigh::Fuel { hop, expand },
        };
    }
    rail.on_hover_text(
        "How short a jump the route may split down to. At the maximum it \
         splits none of them, which is the fewest jumps: the quickest to \
         fly and the thirstiest, a jump at full range costing the drive's \
         whole maximum fuel. At the minimum it is the least fuel there is — \
         as many short hops as the sky offers, ninety-four of a third of a \
         light year to cross three hundred, and slow to work out. Halfway \
         is roughly twice the jumps on half the fuel.",
    );

    // The tie-break, offered only where there are ties of its kind to
    // break. It settles which of the chains of *equal jump count* is
    // taken, which is an ordering the fewest-jumps end of the rail has and
    // no other position on it does — so it is not drawn elsewhere, as
    // `Within` is not drawn where nothing bounds the answer and
    // `Expand nearest` is not drawn for a weighing with no such setting.
    if matches!(how.weigh, Weigh::Jumps | Weigh::Shortest) {
        let mut shortest = matches!(how.weigh, Weigh::Shortest);
        if ui
            .checkbox(&mut shortest, "Shortest")
            .on_hover_text(
                "Of the routes that are the fewest jumps, find the \
                 shortest. Slower: the chains of one length are many, and \
                 knowing which is shortest means walking them.",
            )
            .changed()
        {
            ui.data_mut(|data| data.insert_temp(SHORTEST(), shortest));
            how.weigh = match shortest {
                true => Weigh::Shortest,
                false => Weigh::Jumps,
            };
        }
    }
}

/// What this weighing is allowed to trade, and where the proven ask lives
///
/// **One group per weighing, because the two approximations are not the
/// same kind of thing.** A percent off the fewest is a weight on the
/// estimate and a bounded claim — weighted A\* answers inside
/// `1 + over/100` — and a jump-counted route spends it extremely well: 89
/// expansions at 75% against 5,876 at 95% for the same 45 stops. A cap on
/// what is expanded bounds nothing at all, and it is the only lever that
/// moves a fuel-weighed route: the nearest 64 come within a fiftieth of
/// the proven fuel at three to twenty-seven times the speed.
///
/// **A control that does not apply is not drawn**, which is the one rule
/// this form follows throughout, and the two here are each other's
/// opposite:
///
/// - The percent goes at a shortest hop of *nothing*, where the fuel left
///   to burn has no positive lower bound, so the estimate it multiplies is
///   zero and the rail is provably inert — measured, the identical route
///   in the identical time at 100% and 95%.
/// - `Expand nearest` is drawn *only* there, for the mirror reason: from a
///   quarter of the range up a cap is inert — 15 stops and 1.251 tanks at
///   64 or 512 — where unpriced it is worth 6 to 8 times. And because it
///   is drawn only there, it now *applies* only there: the count the rail
///   remembered used to travel along with a priced ask and decide two
///   percent of the tank at a 5% hop with nothing on screen to say so.
///   See [`Routing::fanout`].
///
/// So the least-fuel end of the trade offers the cap and no percent, and
/// everywhere else offers the percent and no cap. Offering either where it
/// changes nothing is the thing that caused the confusion this form was
/// rebuilt out of.
///
/// **And the proven ask is the top stop of whichever one is drawn** —
/// `Within` at `optimal`, `Expand nearest` at `all` — rather than a tick
/// standing over both. The tick was two controls for one number: it hid
/// these rails, had to remember where they stood to give them back, and
/// left the same state reachable two ways. What makes one control enough
/// is that [`Routing::approximates`] reads the ask: where the percent
/// cannot approximate anything, the cap is the promise.
pub(super) fn approximating(ui: &mut Ui, how: &mut Routing) {
    // The fuel weighing's own lever first, it being the one that moves
    // such a route: what the search looks at rather than what it settles
    // for. The percent under it is the other kind of trade and is dead at
    // this weighing's own far end, so the live control stands first.
    //
    // **At an unpriced hop and nowhere else**, which is where the
    // measurements put it and, since they were taken again, where the cap
    // applies at all. A priced hop takes long jumps and few of them —
    // nine stops at half the range — and from a quarter of the range up a
    // cap changes nothing: 15 stops and 1.251 tanks whether it is 64 or
    // 512. Under that it does change things, which is why the valve rather
    // than this rail's leftovers holds there ([`Routing::fanout`]).
    // Unpriced, the same route is fifty-five short hops and every
    // expansion weighs a thousand candidates. See [`graph::EXPAND`] for
    // where it bites hardest.
    if let Weigh::Fuel { hop: 0, expand } = how.weigh {
        // **A rail over its own stops, not over the counts.** Doubling
        // each step is what the measurements want — eight is 17x the speed
        // of taking every system in range and 64 is within a fiftieth of
        // the fuel, where everything above 64 changes almost nothing — and
        // the last stop is *every* system in reach, which is the graph a
        // proven route searches. So the rail's top stop **is** the proven
        // ask here, there being nothing else at an unpriced hop that could
        // approximate anything: the estimate is zero and weighting zero is
        // zero. See [`Routing::approximates`].
        //
        // Stepping through an index rather than putting the sentinel
        // inside the numbers: a rail from 8 to 1024 that read 1024 as
        // "all" would be lying about a count spheres really do exceed —
        // measured at 1,170 candidates in the bubble at a 100 ly range, so
        // a cap of 1024 is a different search from no cap — and it left
        // 512 as the largest number anybody could ask for, for no reason
        // but that it was the constant this replaced.
        let mut at = STOPS
            .iter()
            .position(|held| *held == expand)
            .unwrap_or(STOPS.len()) as f64;
        let rail = ui.add(
            egui::Slider::new(&mut at, 0.0..=STOPS.len() as f64)
                .step_by(1.)
                .custom_formatter(|held, _| {
                    match STOPS.get(held.round() as usize) {
                        Some(count) => count.to_string(),
                        None => "all".to_owned(),
                    }
                })
                .text("Expand nearest"),
        );
        if rail.changed() {
            let asked =
                STOPS.get(at.round() as usize).copied().unwrap_or_default();
            // **The slack goes with it**, so the two never disagree about
            // whether the answer is proven. A cap of nothing is the proven
            // ask and the percent has to say so — it is what the route
            // carries into its own description ([`Routing::named`]) and
            // what the trade rail hands on if the reader moves off this
            // end — and a cap that bites has to leave the percent
            // standing somewhere it can be read, which is the default's
            // own knee.
            *how = Routing {
                over: match asked {
                    0 => 0,
                    _ if how.over > 0 => how.over,
                    _ => Routing::default().over,
                },
                weigh: Weigh::Fuel { hop: 0, expand: asked },
            };
        }
        rail.on_hover_text(
            "How many of each system's neighbours the search looks at, \
             nearest first. A route flown on the least fuel only ever takes \
             short jumps, so the far half of what a jump reaches is \
             systems it would never use: the nearest 64 come within a \
             fiftieth of the fuel for a fraction of the wait. Fewer is \
             quicker and may miss a long way round. `all` is every system \
             in reach, which is the proven route: nothing is planned or \
             thinned, and across the galaxy that is minutes to hours.",
        );
    }

    // How close to the best it has to come, where there is a best to
    // measure against. Not drawn where there is not: at a shortest hop of
    // nothing the fuel left to burn has no positive lower bound, so the
    // estimate this multiplies is zero and the rail is provably inert —
    // measured, the identical route in the identical time. The cap above
    // is that ask's own promise. See [`bounded`].
    if bounded(how) {
        // **A hundred is a stop of this rail and it reads `optimal`**,
        // which is what the `Optimal` tick used to be. Two controls for
        // one number is what made the tick confusing: it hid the rail,
        // remembered where the rail had been, and put the same state
        // behind two gestures. The top of the rail is the promise now, as
        // the top of the planning rail is.
        let mut optimality = how.optimality() as f64;
        let rail = ui.add(
            egui::Slider::new(&mut optimality, 0.0..=100.0)
                .step_by(5.)
                .custom_formatter(|held, _| match held.round() as u32 {
                    100 => "optimal".to_owned(),
                    within => format!("{within}%"),
                })
                .text("Within"),
        );
        if rail.changed() {
            *how = Routing::at(optimality.round() as u32, how.weigh);
        }
        rail.on_hover_text(
            "How close to the best the route has to come. 95% is a route \
             inside 105% of it, and the slack is what lets the search stop \
             early — the useful range is nowhere near the top: 75% found \
             the same route as 95% in a sixtieth of the expansions. \
             Optimal proves it instead, system by system, which across the \
             galaxy is minutes to hours.",
        );
    }
}

/// Where on the jumps-against-fuel trade a weighing sits, and what it looks
/// at
///
/// The rail's own reading of [`Weigh`]. The fewest-jumps ask is the top of
/// it — a hop of the whole range prices a jump at a tankful, which is the
/// same question — and every position below is the fuel weighing with that
/// hop. A weighing that is not on the trade at all reads as the top, which
/// is where the rail stands when it is not shown.
///
/// The cap comes along so that running the rail down from the top and back
/// does not lose it: the fewest-jumps ask has no such setting of its own,
/// and a reader who set it to eight would not expect to find sixty-four on
/// the way back.
fn traded(weigh: Weigh) -> (u32, u32) {
    match weigh {
        Weigh::Fuel { hop, expand } => (hop, expand),
        Weigh::Jumps | Weigh::Shortest => (100, graph::EXPAND),
    }
}

/// Whether a percent off the best means anything to this ask
///
/// It does not at a shortest hop of nothing: the fuel left to burn has no
/// positive lower bound there, a jump being able to be arbitrarily short,
/// so the estimate the percent multiplies is zero and the answer comes back
/// identical however hard the rail is pushed. Measured over `.index/full`,
/// 100% and 95% returning the same route in 6.1 s and 6.3 s.
fn bounded(how: &Routing) -> bool {
    !matches!(how.weigh, Weigh::Fuel { hop: 0, .. })
}

/// What `Expand nearest` offers, and one stop past the end of it for every
/// system in reach
///
/// Doubling, because that is the shape of what it buys: measured over
/// `.index/full`, eight is seventeen times the speed of taking every system
/// in range and sixty-four is within a fiftieth of the proven fuel, while
/// everything above sixty-four changes almost nothing. Nought is not a stop
/// — it is what [`Weigh::Fuel`] holds for the stop past the last, there
/// being no count that means "all of them".
const STOPS: [u32; 8] = [8, 16, 32, 64, 128, 256, 512, 1024];

/// What the `Plan` rail offers, and the two asks past the end of it
///
/// Percents while a percent is what the plan is leaning by, then the two
/// exact stops, which are asks rather than numbers: `optimal if cheap`
/// tries the exact plan inside [`Tuning::allowance`] and takes a leaned
/// chain where it overruns, and `optimal` pays whatever it costs. Fifty is
/// the far end because that is EDDA's own coarse weight of 1.5
/// (`long_range.rs:52-57`) and nothing measured wanted more.
const PLANS: [u32; 10] = [50, 55, 60, 65, 70, 75, 80, 85, 90, 95];

/// How a long supercharged route is planned, asked for beside the route
///
/// Two settings about *method*, where the two controls above are about the
/// answer: how large a gap between boost stars the coarse plan may string
/// together, and how the jumps that cross one are found. They are here
/// because this is where a route is asked for — they were briefly on a
/// route's own info panel, which reads as controls for the next plot
/// standing under a description of the last one.
///
/// A **gap**, because the two words a reader already has are taken: a *leg*
/// is a part of a multi-stop trip, which the bar says outright, and a *hop*
/// is one jump of a route, which every row says. Neither is this.
///
/// Every one of them was a constant until a ship that jumps 25 light years
/// met a graph tuned against one that jumps 50: the hop reach fell to 200
/// light years, the boost stars stopped being a connected graph, the plan
/// answered nothing in 0.3 ms and the flat search spent **233 s** on the
/// fallback. See [`Tuning`] for what each measured out at.
///
/// **The gap width used to be asked for here and is not any more.** It was
/// a rail from the least reach that connects the cones upward, and the
/// measurements turned it into a trap: a corridor whose own bottleneck is
/// wider than the setting does not fail cheaply — the coarse search never
/// closes on the goal, spends its stall allowance, and the stretch left
/// over becomes one enormous gap for the legs. Measured at 45 Ly, Sol to a
/// system 2 kly out: **60 stops in 32.3 s** at the old 405 Ly default
/// against **32 stops in 6.2 ms** at 495. A wider reach only adds edges to
/// the cone graph, so it can never cost stops, and no reader can be
/// expected to know which corridor wants which number. The plan climbs the
/// reach itself now ([`highway::RUNGS`]) and the floor it starts from is
/// where the curve flattens ([`highway::GAPS_LY`] = 500 Ly).
fn planning(ui: &mut Ui, tune: &mut Tuning) {
    ui.label(
        egui::RichText::new("How the next long supercharged plot is planned")
            .weak(),
    );

    // How hard the plan itself is worked, which is **not** the question
    // the `Within` rail above answers. That one says how good the route
    // has to be; this says how near the fewest hops the chain of cones
    // has to come, and a coarse hop is a lower bound on a gap either way
    // — the plan bounds nothing about the route it leads to, whatever it
    // is weighed with. Until this the plan was leaned by the route's own
    // percent with no way to say the one without the other.
    //
    // **Two exact stops, because they are two answers.** Measured over
    // `.index/full` from Sol at 45 Ly at 80% optimality, leaned against
    // tried against paid: Colonia is 164 stops in 187 ms, 164 in 98 ms,
    // and **154 in 2.83 s**; 22 kly out is 174 in 482 ms, 174 in 467 ms,
    // and **168 in 4.42 s**; and a 2 kly corridor whose exact plan lands
    // inside the allowance is 32 stops in 4.2 ms whichever of the two is
    // asked. So `optimal if cheap` tries the exact plan inside
    // [`Tuning::allowance`] — 2,048 expansions, some 40 ms — and takes a
    // leaned chain where it runs past that; `optimal` pays for it, which
    // is three to six percent of the jumps flown for nine to twenty-nine
    // times the wait.
    //
    // Before the second stop existed there was no way to ask for that
    // difference: the rail only leaned *harder* than exact, and an
    // allowance counted in expansions is not a control.
    //
    // And the rest of the rail is the other direction, for a corridor
    // where even leaning by the route's own percent crawls — EDDA's own
    // answer to the same plateau is a coarse weight of 1.5
    // (`long_range.rs:52-57`).
    //
    // Stepping through an index rather than over the percentages, as
    // `Expand nearest` does: the two stops past the end are asks rather
    // than numbers, and a rail that read `100%` twice would be two
    // handles for one word.
    let mut at = match (tune.planning, tune.allowance) {
        (0, None) => PLANS.len() + 1,
        (0, Some(_)) => PLANS.len(),
        (over, _) => PLANS
            .iter()
            .position(|&within| within == 100 - over.min(100))
            .unwrap_or(PLANS.len()),
    } as f64;
    let plan = ui.add(
        egui::Slider::new(&mut at, 0.0..=PLANS.len() as f64 + 1.)
            .step_by(1.)
            .custom_formatter(|held, _| {
                match PLANS.get(held.round() as usize) {
                    Some(within) => format!("{within}%"),
                    None if held.round() as usize == PLANS.len() => {
                        "optimal if cheap".to_owned()
                    }
                    None => "optimal".to_owned(),
                }
            })
            .text("Plan"),
    );
    if plan.changed() {
        let asked = at.round() as usize;
        *tune = Tuning {
            planning: PLANS.get(asked).map_or(0, |within| 100 - within),
            allowance: match asked > PLANS.len() {
                true => None,
                false => Some(highway::ALLOWANCE),
            },
            ..*tune
        };
    }
    plan.on_hover_text(
        "How near the fewest hops the chain of boost stars has to come. \
         `optimal if cheap` tries the exact plan and keeps it where it \
         lands quickly, falling back to a leaned one where it does not; \
         `optimal` pays for it, which is worth a few stops in a hundred \
         and can be twenty times the wait. Leaning harder is for a \
         corridor where the cones pile up and the plan crawls.",
    );

    // Which of the two the legs are refined by. The hover on the box says
    // what a leg is at all; the two rows inside say what each does with it.
    egui::ComboBox::from_label("Crossing a gap")
        .selected_text(match tune.crossing {
            Crossing::Searched => "Searched",
            Crossing::Stepped => "Stepped",
        })
        .show_ui(ui, |ui| {
            for (kind, said, hint) in [
                (
                    Crossing::Stepped,
                    "Stepped",
                    "Take each jump toward the next boost star, whichever \
                     one is cheapest for the ground it closes — the \
                     nearest where the route counts jumps, the least \
                     thirsty where it counts fuel. Fastest by far; a gap \
                     where no jump gets closer is searched instead.",
                ),
                (
                    Crossing::Searched,
                    "Searched",
                    "Search each gap at the optimality asked for above. \
                     Slower, and the same bargain rather than a better one.",
                ),
            ] {
                ui.selectable_value(&mut tune.crossing, kind, said)
                    .on_hover_text(hint);
            }
        })
        .response
        .on_hover_text(
            "How the jumps from one boost star to the next are worked out",
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::words;

    use bevy::math::DVec3;

    /// The gap width is not a question any more
    ///
    /// **The rail used to open where a hop became expressible** — one
    /// supercharged jump plus one ordinary one — and every notch under the
    /// default could do one thing only: leave the boost stars
    /// disconnected, so there was no plan and the route was searched flat
    /// instead. Raising the floor fixed half of that; the other half is
    /// that no floor is right everywhere, and a corridor whose own
    /// bottleneck is wider fails just as expensively. Measured at 45 Ly,
    /// Sol to a system 2 kly out: 60 stops in **32.3 s** at a 405 Ly reach
    /// against 32 stops in **6.2 ms** at 495.
    ///
    /// So the plan climbs the reach itself and the reader is not asked.
    /// What this pins is that the form no longer offers the number: a
    /// control whose wrong settings are unreachable is better than one
    /// whose wrong settings are a cliff. See
    /// [`galos_route::highway::Highway::plan`].
    #[test]
    fn the_gap_width_is_not_asked_for() {
        let mut tune = Tuning::default();
        let said = words(|ui| planning(ui, &mut tune));

        assert!(
            !said.iter().any(|line| line.contains("Gap")),
            "the gap rail is still offered: {said:?}"
        );
        assert_eq!(
            tune.reach,
            Tuning::default().reach,
            "the form moved a reach nobody asked about",
        );
    }

    /// And both remaining settings are drawn
    ///
    /// A fold that opened onto nothing would leave them unreachable. The
    /// plan's rail reads its two exact stops by name rather than as
    /// percentages of anything — `optimal if cheap` is the default, and
    /// `optimal` is the one that pays for the chain the allowance would
    /// have dropped.
    #[test]
    fn the_planning_fold_holds_every_setting() {
        let mut tune = Tuning::default();
        let said = words(|ui| planning(ui, &mut tune));

        for wanted in ["Plan", "Crossing a gap"] {
            assert!(
                said.iter().any(|line| line == wanted),
                "{wanted} was not offered: {said:?}"
            );
        }
        assert!(
            said.iter().any(|line| line == "optimal if cheap"),
            "the plan rail did not say where it stands: {said:?}"
        );

        // The stop past it, which is the exact plan paid for.
        let mut paid = Tuning { allowance: None, ..Tuning::default() };
        let said = words(|ui| planning(ui, &mut paid));
        assert!(
            said.iter().any(|line| line == "optimal"),
            "the paid stop read as the tried one: {said:?}"
        );

        // And a leaned plan reads as the percentage it is.
        let mut leaned = Tuning { planning: 20, ..Tuning::default() };
        let said = words(|ui| planning(ui, &mut leaned));
        assert!(
            said.iter().any(|line| line == "80%"),
            "a leaned plan did not say its own percent: {said:?}"
        );
    }

    /// A proven route's fold says why there is nothing to plan
    ///
    /// The reported confusion: the fold used to vanish, which reads as a
    /// control that comes and goes rather than as an answer. An answer
    /// that has to be *true* leaves only the flat search — the plan's edges
    /// are lower bounds and bound nothing about the route they lead to — so
    /// there is no plan to work out, and that is now said where the
    /// controls were.
    ///
    /// Both proven asks, because they are two different top stops: the
    /// `Within` rail's, and — at an unpriced hop, where the percent is
    /// inert — the cap rail's `all`. See [`Routing::approximates`].
    #[test]
    fn the_fold_says_why_a_proven_route_is_not_planned() {
        for proven in [
            Routing::at(100, Weigh::Jumps),
            Routing::at(95, Weigh::Fuel { hop: 0, expand: 0 }),
        ] {
            let said = crate::testing::words(|ui| {
                let mut tune = Tuning::default();
                super::planned(ui, proven, &mut tune);
            });
            assert!(
                said.iter().any(|line| line.contains("unused when proven")),
                "{proven:?} did not say the plan is not used: {said:?}"
            );
        }
    }

    /// And what crosses a gap is named for what it promises
    ///
    /// It was `Legs` offering `Leaned`, `Proven` and `Step across`: the
    /// first name collides with a trip's legs, which is what a user means
    /// by the word, and the rest were the algorithm's vocabulary. Two are
    /// left, and they read against the optimality asked for above them.
    #[test]
    fn crossing_a_gap_is_named_for_what_it_promises() {
        let mut tune =
            Tuning { crossing: Crossing::Searched, ..Tuning::default() };
        let searched = words(|ui| planning(ui, &mut tune));
        assert!(searched.iter().any(|line| line == "Searched"), "{searched:?}");

        let mut tune =
            Tuning { crossing: Crossing::Stepped, ..Tuning::default() };
        let nearest = words(|ui| planning(ui, &mut tune));
        assert!(nearest.iter().any(|line| line == "Stepped"), "{nearest:?}");
    }

    /// The trade's ends are the two asks that used to be modes
    ///
    /// **The reported confusion.** "Optimal" beside "least fuel" claimed a
    /// superlative that the shortest-hop rail moved: measured over
    /// `.index/full`, the same corridor proven at a 50% hop burns 0.876 of
    /// a tank against 0.191 at nothing — 4.6 times the least fuel there
    /// is. The dropdown's fewest-jumps entry was the rail's own top stop
    /// besides, a hop of the whole range pricing a jump at a tankful: 3
    /// stops either way. So the two are one rail now, and it says so at
    /// both ends rather than showing a percentage at each.
    #[test]
    fn the_trade_names_its_own_ends() {
        use super::{Weigh, traded};
        use galos_route::graph::EXPAND;

        // The rail's reading of each ask, which is what its handle stands
        // at: the fewest-jumps ask is the top of the trade and not a
        // separate axis.
        assert_eq!(traded(Weigh::Jumps), (100, EXPAND));
        assert_eq!(traded(Weigh::Fuel { hop: 0, expand: 8 }), (0, 8));
        assert_eq!(traded(Weigh::Fuel { hop: 50, expand: 64 }), (50, 64));

        // And what a route made of each says it is, which is where the
        // superlative was wrong: only one end of the rail is the least
        // fuel there is.
        assert_eq!(
            Routing::at(100, Weigh::Fuel { hop: 0, expand: 0 }).named(),
            "optimal, the least fuel there is"
        );
        assert_eq!(
            Routing::at(100, Weigh::Fuel { hop: 50, expand: 0 }).named(),
            "optimal, fuel over jumps at 50% hops"
        );
        assert_eq!(
            Routing::at(100, Weigh::Jumps).named(),
            "optimal, fewest jumps"
        );
    }

    /// The rail says the hop in light years, and which end is which
    ///
    /// A percentage of a range is arithmetic nobody should have to do to
    /// learn whether the route will split its jumps to eleven light years
    /// or to two — and a bare figure at each end says nothing about which
    /// way the trade runs. So the value is the hop the ship would actually
    /// fly, off the range typed above, and the ends are marked.
    #[test]
    fn the_rail_says_the_hop_in_light_years() {
        use super::{Weigh, trading};
        use galos_route::graph::EXPAND;

        // A fifty light year ship, so half the rail is twenty-five.
        let painted = |weigh| {
            let mut how = Routing::at(100, weigh);
            words(|ui| trading(ui, &mut how, Some(50.)))
        };
        let says = |said: &[String], wanted: &str| {
            said.iter().any(|line| line.contains(wanted))
        };

        let top = painted(Weigh::Jumps);
        assert!(says(&top, "50.0 Ly"), "the top was not the range: {top:?}");
        for end in ["least fuel", "fewest jumps"] {
            assert!(says(&top, end), "{end} was not marked: {top:?}");
        }

        let bottom = painted(Weigh::Fuel { hop: 0, expand: EXPAND });
        assert!(says(&bottom, "0.0 Ly"), "{bottom:?}");

        let between = painted(Weigh::Fuel { hop: 50, expand: EXPAND });
        assert!(says(&between, "25.0 Ly"), "half of fifty: {between:?}");

        // Without a ship there is nothing to take a percentage of, and the
        // rail says the percentage rather than a light year figure it
        // cannot work out.
        let unshipped = {
            let mut how = Routing::at(100, Weigh::Fuel { hop: 50, expand: 0 });
            words(|ui| trading(ui, &mut how, None))
        };
        assert!(says(&unshipped, "50%"), "{unshipped:?}");

        // The shortest ask is the rail's top with its tick on, and the tick
        // is offered only there — as `Expand nearest` is offered only to
        // the weighing that has one.
        let ticked =
            |said: &[String]| said.iter().any(|line| line == "Shortest");
        let ties = painted(Weigh::Shortest);
        assert!(says(&ties, "50.0 Ly"), "{ties:?}");
        assert!(ticked(&ties), "no tick at the end that has ties: {ties:?}");
        assert!(ticked(&top), "no tick for the fewest jumps: {top:?}");
        assert!(
            !ticked(&bottom),
            "the tick was offered where it has no ties: {bottom:?}"
        );
    }

    /// A weighing offers the trade it has and not the other one
    ///
    /// **The reported confusion, and it was the form's fault.** One
    /// optimality rail stood for two unlike things: a bounded weight on
    /// the estimate, which a route counted in jumps spends extremely well
    /// — 89 expansions at 75% against 5,876 at 95% for the same 45 stops —
    /// and a cap on what is looked at, which is the only lever that moves
    /// a fuel-weighed route. So the rail read as two sliders doing the
    /// same job, and at a shortest hop of nothing it did no job at all.
    ///
    /// Each weighing names its own now. What is asserted here is the pair
    /// of absences as much as the presences: a control that does nothing
    /// is the thing this form is not allowed to show.
    #[test]
    fn a_weighing_offers_only_the_trade_it_has() {
        use super::{Weigh, approximating};
        use galos_route::graph::EXPAND;

        let offered = |weigh| {
            let mut how = Routing::at(95, weigh);
            words(|ui| approximating(ui, &mut how))
        };
        let says = |said: &[String], wanted: &str| {
            said.iter().any(|line| line.contains(wanted))
        };

        // Counted in jumps: a percent off the fewest, and nothing about
        // neighbours — the same cap tightened there made a charged three
        // thousand light year crossing slower for the same stops.
        let jumps = offered(Weigh::Jumps);
        assert!(says(&jumps, "Within"), "no percent for jumps: {jumps:?}");
        assert!(
            !says(&jumps, "Expand"),
            "a jumps route was offered a cap it cannot use: {jumps:?}"
        );

        // A priced hop: the percent, and no cap — inert from a quarter of
        // the range up, 15 stops and 1.251 tanks at 64 or 512.
        let priced = offered(Weigh::Fuel { hop: 50, expand: EXPAND });
        assert!(says(&priced, "Within"), "no percent at a priced hop");
        assert!(
            !says(&priced, "Expand nearest"),
            "a cap where it measures nothing: {priced:?}"
        );
        // And because it is not drawn there it does not apply there: the
        // count this rail leaves behind used to decide two percent of the
        // tank at a 5% hop unseen. See [`Routing::fanout`].
        assert_eq!(
            Routing::at(95, Weigh::Fuel { hop: 5, expand: 8 }).named(),
            Routing::at(95, Weigh::Fuel { hop: 5, expand: 1024 }).named(),
            "a priced hop still reads the cap the rail remembered",
        );

        // And unpriced: the cap, and no percent, each for the reason the
        // other is there.
        let fuel = offered(Weigh::Fuel { hop: 0, expand: EXPAND });
        assert!(says(&fuel, "Expand nearest"), "no cap for fuel: {fuel:?}");
        assert!(says(&fuel, "64"), "the cap did not say its own count");
        assert!(
            !says(&fuel, "Within"),
            "a percent where nothing bounds the answer: {fuel:?}"
        );

        // Every count on the rail is a real one, including the largest:
        // spheres do exceed a thousand candidates — measured at 1,170 in
        // the bubble at a 100 ly range — so a cap of 1024 is a different
        // search from no cap and may not be read as "all of them".
        let most = offered(Weigh::Fuel { hop: 0, expand: 1024 });
        assert!(says(&most, "1024"), "the largest count was not offered");
        assert!(
            !most.iter().any(|line| line == "all"),
            "a count was painted as all of them: {most:?}"
        );

        // And the rail's top stop says what it is rather than a number
        // that would read as a thousand and twenty-four's big brother:
        // every system the jump reaches, which is the graph a proven route
        // searches.
        let all = offered(Weigh::Fuel { hop: 0, expand: 0 });
        // Whole, so it cannot pass on a stray "all" inside some other
        // label that happens to be painted beside it.
        assert!(
            all.iter().any(|line| line == "all"),
            "the top stop was a number: {all:?}"
        );
    }

    /// And the percent is not drawn where it means nothing
    ///
    /// At a shortest hop of nothing there is no positive lower bound on the
    /// fuel left to burn, so the estimate the percent multiplies is zero
    /// and pushing the rail changes neither the route nor the wait —
    /// measured, 100% and 95% coming back identical in 6.1 s and 6.3 s. A
    /// rail that cannot move the answer is worse than no rail: it is the
    /// form promising a trade it cannot make, which is what sent a reader
    /// looking for the difference between two identical plots.
    #[test]
    fn the_percent_goes_where_no_bound_exists() {
        use super::{Weigh, bounded};
        use galos_route::graph::EXPAND;

        let unpriced = Routing::at(95, Weigh::Fuel { hop: 0, expand: EXPAND });
        assert!(!bounded(&unpriced), "an unpriced hop claimed a bound");

        for weigh in [
            Weigh::Jumps,
            Weigh::Shortest,
            Weigh::Fuel { hop: 5, expand: EXPAND },
        ] {
            let how = Routing::at(95, weigh);
            assert!(bounded(&how), "{weigh:?} lost its bound");
        }

        // And gone from the form there, where every other dead control is.
        let said = words(|ui| {
            let mut how = unpriced;
            super::approximating(ui, &mut how);
        });
        assert!(
            !said.iter().any(|line| line.contains("Within")),
            "the percent was offered where it cannot move: {said:?}"
        );
        // Drawn wherever it can, which is the half that would otherwise
        // pass by the form having no controls at all.
        let priced = words(|ui| {
            let mut how = Routing::at(95, Weigh::Fuel { hop: 5, expand: 0 });
            super::approximating(ui, &mut how);
        });
        assert!(
            priced.iter().any(|line| line.contains("Within")),
            "the percent went missing at a priced hop: {priced:?}"
        );
    }

    /// The proven ask is the top stop of whichever rail applies
    ///
    /// **What the `Optimal` tick used to be.** It was a second control
    /// over one number: ticking it hid the rails, unticking it had to
    /// remember where they stood, and the same state was reachable two
    /// ways. The top of each rail says it now — `optimal` where the
    /// percent bites, `all` where it does not — and the two cannot
    /// disagree, because [`Routing::approximates`] reads the ask rather
    /// than the percent alone.
    #[test]
    fn the_top_of_the_rail_is_the_proven_ask() {
        use super::approximating;
        use galos_route::graph::EXPAND;

        // Where the percent bites, its own rail carries the word.
        let said = words(|ui| {
            let mut how = Routing::at(100, Weigh::Jumps);
            approximating(ui, &mut how);
        });
        assert!(
            said.iter().any(|line| line == "optimal"),
            "the percent's top stop did not say what it is: {said:?}"
        );
        assert!(
            said.iter().any(|line| line == "Within"),
            "the rail went missing at its own top stop: {said:?}"
        );

        // And where it does not, the cap does the promising: `all` is the
        // whole sphere, which is the graph a proven route searches.
        let all = Routing::at(95, Weigh::Fuel { hop: 0, expand: 0 });
        assert!(
            !all.approximates(),
            "every system in range still claimed an approximation",
        );
        let capped = Routing::at(100, Weigh::Fuel { hop: 0, expand: EXPAND });
        assert!(
            capped.approximates(),
            "a capped search claimed to be proven for want of slack",
        );

        // And what the trade rail lands on when a proven ask runs down to
        // the fuel end: the cap's own top stop, so the promise survives
        // the trip rather than being lost to whatever count the rail
        // remembered.
        assert!(
            !Routing::at(100, Weigh::Fuel { hop: 0, expand: 0 }).approximates(),
            "the fuel end of a proven ask was not proven",
        );
    }

    /// A plot short enough that the form has nothing to suggest
    ///
    /// Which is every ordinary plot: the guard's own rows are milliseconds
    /// to a second or two, and a form that offered a cheaper setting after
    /// each of them would be a form nobody reads.
    #[test]
    fn a_quick_plot_is_offered_nothing() {
        let dear = Tuning {
            crossing: Crossing::Searched,
            planning: 0,
            allowance: None,
            ..Tuning::default()
        };
        assert_eq!(
            quicker_by(
                PATIENCE - std::time::Duration::from_millis(1),
                &Routing::FEWEST,
                &dear,
                Drive::Standard,
            ),
            None,
        );
    }

    /// And a long one is told what is costing it, dearest setting first
    ///
    /// One suggestion and the dearest one: searching every gap is an order
    /// of magnitude over the rest, so a plot doing that hears about that
    /// and not about the two rails it also has set to proven. What is left
    /// once the dear settings are the cheap ones is nothing — a wait with
    /// no advice attached, which over a galaxy this size is the truth.
    #[test]
    fn a_long_plot_is_told_which_setting_to_turn_down() {
        let long = PATIENCE;
        let searched = Tuning {
            crossing: Crossing::Searched,
            planning: 0,
            allowance: None,
            ..Tuning::default()
        };
        let said = |tune: &Tuning, how: &Routing, drive| {
            quicker_by(long, how, tune, drive).unwrap_or("nothing")
        };

        assert!(
            said(&searched, &Routing::FEWEST, Drive::Standard)
                .starts_with("Crossing a gap"),
            "the dearest setting went unsaid: {}",
            said(&searched, &Routing::FEWEST, Drive::Standard),
        );
        // Gaps stepped, so the plan is next — and only for a drive that can
        // use a cone, there being no plan at all without one.
        let exact = Tuning { planning: 0, allowance: None, ..searched };
        let stepped = Tuning { crossing: Crossing::Stepped, ..exact };
        assert!(
            said(&stepped, &Routing::FEWEST, Drive::Standard)
                .starts_with("Plan"),
            "the plan went unsaid: {}",
            said(&stepped, &Routing::FEWEST, Drive::Standard),
        );
        assert!(
            said(&stepped, &Routing::FEWEST, Drive::Unaided)
                .starts_with("Within"),
            "an unaided plot was sent to the plan it does not use: {}",
            said(&stepped, &Routing::FEWEST, Drive::Unaided),
        );
        // Everything cheap already: nothing to offer.
        assert_eq!(
            said(&Tuning::default(), &Routing::default(), Drive::Standard),
            "nothing",
        );
    }

    /// And the readout draws it, where a plot has been running that long
    ///
    /// The choice is [`quicker_by`]'s and the two tests above are about
    /// that; this is the one thing they cannot say, which is that a reader
    /// waiting on a plot has it in front of them.
    #[test]
    fn a_long_plot_draws_the_advice() {
        let mut searching = Frontiers::default();
        let asked = std::time::Instant::now()
            .checked_sub(PATIENCE + std::time::Duration::from_secs(1))
            .expect("a moment before now");
        searching.watch(
            crate::map::galaxy::fetch::FetchIndex::Route(
                "START".into(),
                "END".into(),
                "50".into(),
                None,
                Drive::Standard,
                Routing::default(),
                Tuning::default(),
            ),
            graph::Frontier::between(DVec3::ZERO, DVec3::new(6400., 0., 0.)),
            asked,
        );
        let searched =
            Tuning { crossing: Crossing::Searched, ..Tuning::default() };

        let said = words(|ui| {
            searching_says(
                ui,
                &searching,
                &Routing::default(),
                &searched,
                Drive::Standard,
            )
        });

        assert!(
            said.iter().any(|line| line.starts_with("Crossing a gap")),
            "the form said nothing about the setting costing the wait: \
             {said:?}",
        );
        // And nothing where the settings are already the cheap ones: the
        // wait is then the galaxy's, not a setting's.
        let quiet = words(|ui| {
            searching_says(
                ui,
                &searching,
                &Routing::default(),
                &Tuning::default(),
                Drive::Standard,
            )
        });
        assert!(
            !quiet.iter().any(|line| line.starts_with("Crossing a gap")),
            "{quiet:?}",
        );
    }
}
