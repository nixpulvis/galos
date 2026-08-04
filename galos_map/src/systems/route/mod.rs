use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::systems::filter::{Filter, Filters};
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use super::system_to_vec;

pub fn plugin(app: &mut App) {
    app.add_message::<Plotted>();
    // After the fetch it answers has been drawn, and before the camera is
    // pointed, since where it asks the camera to go is what `move_camera`
    // then works out.
    app.add_systems(
        Update,
        plotted.in_set(MapSet::Populate).after(super::spawn::spawn),
    );
}

#[derive(Component)]
pub struct Route;

/// A route that has landed and been drawn
///
/// Written where the fetch is collected, since that is the one place the
/// systems it runs through are in hand, and answered here so that what a
/// route does to the map is in one place rather than threaded through the
/// system that draws stars.
#[derive(Message, Debug)]
pub struct Plotted {
    /// The two ends, as the database spells them
    pub label: String,
    /// Every system it runs through, by address, sorted
    pub systems: Vec<i64>,
    /// The middle of what it spans
    pub middle: DVec3,
    /// How far it reaches from there, in light years
    pub extent: f32,
}

/// Show a route that has just been plotted
///
/// Three things at once, all of them the same thought: look at the whole of
/// it, reach far enough to hold the whole of it, and pick the whole of it
/// out from everything else.
///
/// The spyglass is set rather than left alone because a route is usually
/// longer than whatever the user was looking at when they asked for it, and a
/// route drawn as a line running out through the edge of an unchanged
/// spyglass is a route with no systems on it.
fn plotted(
    mut plotted: MessageReader<Plotted>,
    mut camera: MessageWriter<MoveCamera>,
    mut spyglass: ResMut<Spyglass>,
    mut filters: ResMut<Filters>,
) {
    for route in plotted.read() {
        camera.write(MoveCamera {
            position: Some(route.middle),
            framing: Some(route.extent),
        });

        // Measured from the middle, which is where the camera is going, so
        // what the spyglass holds is what the camera is about to see.
        spyglass.radius = route.extent.max(MIN_REACH);

        filters.replace(Filter::Route {
            label: route.label.clone(),
            systems: route.systems.clone(),
        });
    }
}

/// The least a route may pull the spyglass in to
///
/// A route between two neighbours spans a few light years, and a spyglass
/// drawn in that far shows the two ends and nothing around them. The map is
/// left with at least the reach it opens with.
const MIN_REACH: f32 = 10.;

pub mod fetch;
pub mod spawn;

/// A list of points that will have a line drawn between each consecutive points
#[derive(Debug, Clone)]
struct LineStrip {
    points: Vec<Vec3>,
}

impl From<LineStrip> for Mesh {
    fn from(line: LineStrip) -> Self {
        Mesh::new(
            // This tells wgpu that the positions are a list of points
            // where a line will be drawn between each consecutive point
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::RENDER_WORLD,
        )
        // Add the point positions as an attribute
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, line.points)
    }
}
