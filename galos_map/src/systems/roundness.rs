//! How round a sphere on the map is drawn
//!
//! Everything round on the map is drawn from one of a handful of spheres, and
//! which one is asked afresh every frame from how large the thing is on
//! screen. A system's mark seen from across the galaxy and a planet filling
//! the view are the two ends of the same question, so one ladder answers both.

use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, init_meshes);
}

/// The spheres anything round is drawn with
///
/// Icosphere subdivisions against the radius in pixels each is asked for at,
/// coarsest first. What asks is the silhouette: shading reads the smooth
/// normals whatever the count, and the outline is the polygon the faces
/// actually make. That polygon is off the circle it stands for by
/// `R * pi^2 / (2n^2)`, where `n` is how many segments run round the middle,
/// which for a sphere subdivided `s` times is about `6(s + 1)`.
///
/// So each rung holds four times the faces of the one below it and serves four
/// times the size, and the figures are where the rung below passes about a
/// quarter of a pixel out. The bottom is a bare icosahedron, which is all a
/// half pixel dot has ever needed, and the top is a body filling a tall screen.
const LADDER: [(u32, f32); 7] =
    [(0, 0.), (1, 2.), (3, 8.), (7, 32.), (15, 128.), (31, 512.), (63, 2048.)];

/// How far a sphere shrinks past what raised it before it drops back
///
/// A sphere sitting on a switch point would otherwise swap mesh every frame
/// the camera breathed, and every swap tells the renderer to extract that
/// entity again. The gap is wide enough to cost nothing to look at, since
/// either rung is under a pixel wrong anywhere inside it.
const HOLD: f32 = 0.7;

/// The spheres, built once and shared by everything drawn as one
#[derive(Resource)]
pub struct Roundness([Handle<Mesh>; LADDER.len()]);

impl Roundness {
    /// Where a sphere starts, before anything has measured how large it draws
    ///
    /// Whichever sizing system owns the thing fits it on the frame it is
    /// spawned, so this is only somewhere to begin.
    pub fn coarsest(&self) -> Handle<Mesh> {
        self.0[0].clone()
    }

    /// The sphere something drawn `pixels` across wants, holding `now`
    pub fn at<'a>(
        &'a self,
        now: &Handle<Mesh>,
        pixels: f32,
    ) -> &'a Handle<Mesh> {
        &self.0[settled(self.rung_of(now), pixels)]
    }

    /// Which rung `mesh` is, and the coarsest for anything off the ladder
    fn rung_of(&self, mesh: &Handle<Mesh>) -> usize {
        self.0.iter().position(|rung| rung == mesh).unwrap_or(0)
    }
}

/// Which rung something drawn `pixels` across settles at, coming from `now`
///
/// Up as far as the size asks for and down only past [`HOLD`], so the two
/// walks cannot both move and the rung between two switch points is whichever
/// was reached first.
fn settled(now: usize, pixels: f32) -> usize {
    let mut rung = now.min(LADDER.len() - 1);
    while rung + 1 < LADDER.len() && pixels >= LADDER[rung + 1].1 {
        rung += 1;
    }
    while rung > 0 && pixels < LADDER[rung].1 * HOLD {
        rung -= 1;
    }

    rung
}

fn init_meshes(mut assets: ResMut<Assets<Mesh>>, mut commands: Commands) {
    commands.insert_resource(Roundness(LADDER.map(|(subdivisions, _)| {
        assets.add(Sphere::new(1.).mesh().ico(subdivisions).unwrap())
    })));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder climbs, in faces and in the size that asks for them
    ///
    /// What [`settled`] rests on. Both of its walks stop at the first rung
    /// that answers, which only finds the right one if the rungs are in order.
    #[test]
    fn the_ladder_climbs() {
        for pair in LADDER.windows(2) {
            let (below, at) = (pair[0], pair[1]);

            assert!(
                below.0 < at.0,
                "ico({}) is no finer than ico({})",
                at.0,
                below.0
            );
            assert!(
                below.1 < at.1,
                "{} pixels is no larger than {}",
                at.1,
                below.1
            );
        }
    }

    /// A sphere settles at the rung its size asks for, from either direction
    ///
    /// The same rung whether it was reached climbing or dropping. A sphere
    /// growing and one shrinking past the same size are the same sphere, and
    /// the dead band is meant to hold what is between switch points rather
    /// than to leave the ends of the ladder disagreeing about them.
    #[test]
    fn a_size_settles_where_it_asks() {
        let top = LADDER.len() - 1;
        for (rung, (_, size)) in LADDER.into_iter().enumerate() {
            assert_eq!(settled(0, size), rung, "climbing to {size} pixels");
            assert_eq!(settled(top, size), rung, "dropping to {size} pixels");
        }
    }

    /// Nothing is drawn finer or coarser than the ladder goes
    #[test]
    fn the_ends_of_the_ladder_hold() {
        assert_eq!(settled(0, 0.), 0);
        assert_eq!(settled(0, -1.), 0);
        assert_eq!(settled(0, 1e9), LADDER.len() - 1);
    }

    /// A sphere sitting on a switch point does not swap back and forth
    ///
    /// The one thing the dead band is for. A camera that never quite stops
    /// moving would otherwise hand the renderer every shell in the sky to
    /// extract again on the frame it drifted a pixel.
    #[test]
    fn a_sphere_on_a_switch_point_holds() {
        for (rung, (_, size)) in LADDER.into_iter().enumerate().skip(1) {
            let raised = settled(rung - 1, size);
            assert_eq!(raised, rung, "{size} pixels did not raise it");

            assert_eq!(
                settled(raised, size * 0.99),
                rung,
                "a hair under {size} pixels dropped it"
            );
            assert_eq!(
                settled(raised, size * HOLD * 0.99),
                rung - 1,
                "past the band under {size} pixels held it"
            );
        }
    }
}
