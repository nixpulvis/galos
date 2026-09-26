//! What a route asks of a fuel tank: the star each stop drops in at, and
//! where along the way it can scoop

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use std::collections::{HashMap, HashSet};

/// The arrival star's class for the systems a panel lists
///
/// **A list is a finite thing, so it is looked up rather than guessed at.**
/// The map has no star class resident for every system — the payload
/// carries six temperature buckets and a route's stops are built from the
/// names table, which carries none — but a panel lists tens or hundreds of
/// systems, not two hundred million, and the index answers one address at a
/// time ([`galos_index::Source::bodies`]). So the classes are read for
/// exactly what is listed, off the task pool, once.
///
/// Held per address rather than per panel: two panels listing the same
/// system ask one question between them, and a class does not change while
/// the map is open.
///
/// [`None`] for a system nothing has scanned, which is most of the galaxy —
/// and said as nothing rather than as a guess.
#[derive(Resource, Default)]
pub struct StarClasses {
    known: HashMap<i64, Option<String>>,
    /// What has been asked and not yet answered, so a frame does not ask
    /// again while the pool is still reading.
    asked: HashSet<i64>,
    /// The reads under way, each answering for one address.
    reading: Vec<(i64, Task<Option<String>>)>,
}

impl StarClasses {
    /// The class where it is known, and [`None`] where it is not — whether
    /// because nothing is scanned or because the read has not landed.
    pub fn of(&self, address: i64) -> Option<&str> {
        self.known.get(&address)?.as_deref()
    }

    /// Ask about every address in `listed` that has not been asked about
    ///
    /// One read a system, off the pool: the index's own answer is a file per
    /// system, so there is nothing to batch.
    pub(super) fn ask(
        &mut self,
        listed: impl IntoIterator<Item = i64>,
        transport: &crate::map::index::Transport,
    ) {
        for address in listed {
            if self.known.contains_key(&address) || !self.asked.insert(address)
            {
                continue;
            }
            let reading = transport.0.clone();
            self.reading.push((
                address,
                AsyncComputeTaskPool::get().spawn(async move {
                    let inside =
                        reading.bodies(address).await.unwrap_or_default();
                    arrival_class(&inside)
                }),
            ));
        }
    }

    /// Take in whatever has landed.
    pub(super) fn poll(&mut self) {
        self.reading.retain_mut(|(address, task)| match bevy::tasks::block_on(
            bevy::tasks::futures_lite::future::poll_once(task),
        ) {
            Some(class) => {
                self.known.insert(*address, class);
                false
            }
            None => true,
        });
    }
}

/// The class of the star a ship drops in at
///
/// The index's own rule, not another one beside it
/// ([`galos_index::derive::arrival_class`]): the star nearest the arrival
/// point, ties broken by body id. It matters that this is the same rule the
/// published boost table was derived by — a panel that read the primary as
/// "the star that goes round nothing" would name a different star in a close
/// pair than the table saying whether that system can supercharge, and the
/// two readings would disagree about the same system on the same screen.
fn arrival_class(inside: &galos_index::meta::SystemBodies) -> Option<String> {
    galos_index::derive::arrival_class(inside).map(str::to_owned)
}

/// How a jump's fuel goes with its length, drive by drive
///
/// **What the map can say about fuel, and what it cannot.** The game's cost
/// of a jump is `multiplier x (distance x mass / optimal mass) ^ p`, which
/// wants the drive's class and rating, the hull, the cargo and the tank —
/// none of which the map is told. Dividing by the drive's own maximum
/// cancels nearly all of it, because a ship's *range* is by definition the
/// distance at which a jump costs that whole maximum:
///
/// ```text
/// fuel(d) / max fuel per jump = (d / range) ^ p
/// ```
///
/// The multiplier cancels, the laden mass cancels, the optimal mass
/// cancels. What is left is the jump against the range the route was
/// plotted at, and `p`, which is the drive's **class** and nothing else.
///
/// The exponent could be bounded — `p = 2.0` is the dearest any drive can
/// be, since a jump is no longer than the range — and a ceiling over every
/// drive in the game was what the panel said first. It read as a fact and
/// was not one: **nobody flies a bound over all drives, they fly a class 5
/// with a specific tank**, and a figure a third too high for their ship is
/// worse than no figure. So the panel states the distance, which is exact,
/// and this rule beside it, which is what turns the distance into fuel for
/// the ship the reader actually has.
///
/// The tank in these units is its capacity divided by the max fuel per
/// jump, both of which the outfitting screen states.
pub(super) fn fuel_rule() -> String {
    let classes = POWERS
        .iter()
        .map(|(class, power)| format!("{class} → {power:.2}"))
        .collect::<Vec<String>>()
        .join(", ");

    format!(
        "A jump of d costs (d / range) ^ p of the drive's maximum fuel, \
         where p is its class: {classes}. The tank holds its capacity \
         divided by that maximum, so a 32 t tank at 0.90 t a jump is 35 \
         jumps' worth at full range — and far more at half of it, fuel \
         going as the square of the jump at least."
    )
}

/// The exponent of each frame shift drive class
///
/// The one ship fact the cancellation above leaves, and it depends on the
/// drive's size alone: a class 2 is the dearest per light year and a class
/// 8 the cheapest. Written out rather than interpolated, since it is a
/// table the game states and not a line anything derived.
const POWERS: [(u8, f64); 7] = [
    (2, 2.00),
    (3, 2.15),
    (4, 2.30),
    (5, 2.45),
    (6, 2.60),
    (7, 2.75),
    (8, 2.90),
];

/// What a route asks of a fuel tank, as far as the classes read say
///
/// The longest run of stops a ship crosses with **nothing to scoop**, and
/// which stop it sets out from. A fuel scoop takes hydrogen off the main
/// sequence and off nothing else (`galos_index::meta::scoopable`), so a
/// stretch of white dwarfs, brown dwarfs and black holes is a stretch the
/// ship crosses on the fuel it had — and where that stretch is longer than
/// the tank, the route is not a slower route, it is a stranded ship.
///
/// Said as the **distance** the stretch takes to cross, not as a count of
/// stops and not as a fuel figure. A count is the wrong reading on its own:
/// six short hops and two long jumps are the same count and nothing like
/// the same fuel, and what strands a ship is the fuel. A fuel figure is the
/// wrong reading too, because the map is not told the ship — see
/// [`fuel_rule`], which the line carries on hover so the reader can turn
/// the distance into their own drive's answer.
///
/// **A class nothing has read is not counted as unscoopable.** The run is
/// the stops *known* to have nothing to scoop, and the stops with no class
/// on record are counted separately and said separately — a route across
/// unexplored space is mostly unread, and reading that as a starving route
/// would condemn every galactic plot. So the reading is a floor: at least
/// this far, with this many unknown.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Scooping {
    /// The longest run of stops known to have nothing to scoop
    run: usize,
    /// Which stop that run sets out from
    from: Option<String>,
    /// How far crossing that run takes, in light years
    ///
    /// From the last star that could refuel the ship to the next one: the
    /// jumps out of the one up to and including the jump that lands on the
    /// other, a tank filled at the one having to reach the other. Nothing
    /// where the route states no distances.
    across: Option<f64>,
    /// How many of the route's stops have no class on record
    unread: usize,
}

impl Scooping {
    /// What a route's stops come to, walked in the order they are flown
    ///
    /// Each stop is its name, the class of the star waiting there, and how
    /// far the jump onto it was. A run ends where a scoopable star arrives,
    /// since the tank is full again there — and the jump that *landed* on
    /// that star was flown on the old tank, so it belongs to the run it
    /// ends. An unread stop ends a run as well rather than extending it:
    /// the run is what is known, and the unknowns are said beside it.
    pub(super) fn of<'a>(
        stops: impl IntoIterator<Item = (&'a str, Option<&'a str>, Option<f64>)>,
    ) -> Scooping {
        let mut said = Scooping::default();
        let mut run = 0;
        let mut from: Option<&str> = None;
        // The jumps of the run standing, and the one that will land on the
        // next star able to refuel the ship.
        let mut jumps: Vec<f64> = Vec::new();

        // A finished run, against the longest one held. Weighed only where
        // a run *ends*, since the jump that lands on the star which refuels
        // the ship belongs to the run it ends and the run's length does not
        // grow to take it.
        let mut settle = |run: usize, from: Option<&str>, jumps: &[f64]| {
            if run > said.run {
                said.run = run;
                said.from = from.map(str::to_owned);
                said.across = match jumps.is_empty() {
                    true => None,
                    false => Some(jumps.iter().sum()),
                };
            }
        };

        for (name, class, jump) in stops {
            match class {
                Some(class) if galos_index::meta::scoopable(class) => {
                    // The jump that arrived here was flown before the tank
                    // was filled here, so it is the run's to pay for.
                    jumps.extend(jump);
                    settle(run, from, &jumps);
                    run = 0;
                    jumps.clear();
                }
                Some(_) => {
                    if run == 0 {
                        from = Some(name);
                        jumps.clear();
                    }
                    run += 1;
                    jumps.extend(jump);
                }
                // Nothing is known about refuelling here, so the run of
                // stops known to starve ends — without this jump, which
                // may well be paid for out of a tank filled here.
                None => {
                    said.unread += 1;
                    settle(run, from, &jumps);
                    run = 0;
                    jumps.clear();
                }
            }
        }
        // A route that ends mid-run: the ship still had to get there.
        settle(run, from, &jumps);
        said
    }

    /// What the panel says of it, or nothing where there is nothing to say
    ///
    /// Nothing for a route that can refuel at every stop it is known to
    /// pass, which is most routes through settled space: a line saying a
    /// route is fine is a line read every time to learn nothing.
    pub(super) fn said(&self) -> Option<String> {
        let run = match self.run {
            0 => return None,
            1 => "1 stop".to_owned(),
            run => format!("{run} stops in a row"),
        };
        let where_from = match &self.from {
            Some(from) => format!(", from {from}"),
            None => String::new(),
        };
        // How far it is to cross, which is the figure a reader can turn
        // into their own drive's fuel. See [`fuel_rule`].
        let across = match self.across {
            Some(across) => format!(", {across:.1} Ly to cross"),
            None => String::new(),
        };
        let unread = match self.unread {
            0 => String::new(),
            1 => ", 1 stop unread".to_owned(),
            unread => format!(", {unread} stops unread"),
        };
        Some(format!("nothing to scoop at {run}{where_from}{across}{unread}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The longest stretch a route crosses with nothing to scoop
    ///
    /// The reading that says whether a plotted route is flyable at all
    /// rather than merely long: a scoop takes hydrogen off the main
    /// sequence, so a run of white dwarfs and brown dwarfs is crossed on
    /// the fuel the ship set out with.
    #[test]
    fn a_route_says_its_longest_stretch_with_nothing_to_scoop() {
        let said = Scooping::of([
            ("SOL", Some("G"), None),
            ("ONE", Some("DA"), Some(50.)),
            ("TWO", Some("Y"), Some(50.)),
            ("THREE", Some("H"), Some(50.)),
            ("SCOOPABLE", Some("K"), Some(50.)),
            ("FOUR", Some("N"), Some(50.)),
        ]);

        assert_eq!(said.run, 3, "the run was miscounted: {said:?}");
        assert_eq!(said.from.as_deref(), Some("ONE"));
        assert_eq!(said.unread, 0);
        // Four jumps of fifty: the three onto the starving stops and the
        // one that lands on the star which can refuel the ship.
        assert_eq!(said.across, Some(200.));
        assert_eq!(
            said.said().as_deref(),
            Some(
                "nothing to scoop at 3 stops in a row, from ONE, \
                 200.0 Ly to cross"
            ),
        );
    }

    /// The stretch is measured in light years, not in stops
    ///
    /// Two stretches of the same count and nothing like the same crossing:
    /// what a tank answers is the distance, fuel going as the square of a
    /// jump at least, and the count alone says neither.
    #[test]
    fn a_stretch_is_said_in_light_years() {
        let stretch = |jump: f64| {
            Scooping::of([
                ("ONE", Some("DA"), Some(jump)),
                ("TWO", Some("DA"), Some(jump)),
                ("SCOOPABLE", Some("G"), Some(jump)),
            ])
        };

        assert_eq!(stretch(50.).run, stretch(25.).run, "the counts differ");
        assert_eq!(stretch(50.).across, Some(150.));
        assert_eq!(stretch(25.).across, Some(75.));
    }

    /// A route that states no distances says the stops and nothing else
    ///
    /// There is nothing to add up. A list drawn with no camera to measure
    /// from still names its stops, and saying nothing about the crossing is
    /// the honest half of that.
    #[test]
    fn a_route_with_no_distances_says_only_its_stops() {
        let said = Scooping::of([
            ("ONE", Some("DA"), None),
            ("TWO", Some("DA"), None),
        ]);

        assert_eq!(said.run, 2);
        assert_eq!(said.across, None);
        assert_eq!(
            said.said().as_deref(),
            Some("nothing to scoop at 2 stops in a row, from ONE"),
        );
    }

    /// The fuel rule names every drive class the game has
    ///
    /// What the panel says instead of a fuel figure of its own. A reader
    /// with a class 5 fitted needs the exponent for a class 5, and a
    /// ceiling over all of them read as a fact about their ship — so the
    /// rule is stated and the arithmetic left to the one person who knows
    /// what is fitted.
    #[test]
    fn the_fuel_rule_names_each_drive_class() {
        let said = fuel_rule();

        for (class, power) in POWERS {
            assert!(
                said.contains(&format!("{class} → {power:.2}")),
                "class {class} went unsaid: {said}",
            );
        }
        assert!(said.contains("(d / range) ^ p"), "{said}");
    }

    /// A route that can refuel everywhere says nothing at all
    ///
    /// A line saying a route is fine is a line read every time to learn
    /// nothing. Most routes through settled space are this.
    #[test]
    fn a_route_that_can_refuel_anywhere_is_not_remarked_on() {
        let said = Scooping::of([
            ("SOL", Some("G"), None),
            ("BARNARD", Some("M"), Some(6.)),
        ]);

        assert_eq!(said.run, 0);
        assert_eq!(said.said(), None, "{said:?}");
    }

    /// A class nothing has read is not counted as a starving stop
    ///
    /// The honest half. A route across unexplored space is mostly unread,
    /// and reading unknown as unscoopable would condemn every galactic
    /// plot — so the run is what is *known* to have nothing to scoop, and
    /// the unknowns are counted beside it. The reading is a floor.
    #[test]
    fn an_unread_stop_is_said_rather_than_assumed() {
        let said = Scooping::of([
            ("SOL", Some("G"), None),
            ("ONE", Some("DA"), Some(50.)),
            ("UNREAD", None, Some(50.)),
            ("TWO", Some("DA"), Some(50.)),
        ]);

        assert_eq!(said.run, 1, "an unread stop extended the run: {said:?}");
        assert_eq!(said.unread, 1);
        assert_eq!(
            said.said().as_deref(),
            Some(
                "nothing to scoop at 1 stop, from ONE, 50.0 Ly to cross, \
                 1 stop unread"
            ),
        );
    }

    /// And a giant is the same star grown, so it still refuels a ship
    ///
    /// The case a first-letter rule gets wrong in the other direction: `MS`
    /// is an S-type star and not an `M` dwarf, while `M_RedGiant` is.
    #[test]
    fn a_giant_refuels_and_an_s_type_does_not() {
        let said = Scooping::of([
            ("GIANT", Some("M_RedGiant"), None),
            ("S TYPE", Some("MS"), Some(50.)),
            ("GIANT TOO", Some("K_OrangeGiant"), Some(50.)),
        ]);

        assert_eq!(said.run, 1, "{said:?}");
        assert_eq!(said.from.as_deref(), Some("S TYPE"));
    }
}
