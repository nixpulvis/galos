use bevy::prelude::*;
use elite_journal::Allegiance;
use galos_db::systems::System as DbSystem;

#[derive(Component)]
pub struct System {
    address: i64,
    name: String,
    population: u64,
    allegiance: Option<Allegiance>,
}

pub mod fetch;
pub use self::fetch::*;

pub mod scale;
pub use self::scale::*;

pub mod spawn;
pub use self::spawn::*;

pub mod route;
pub use self::route::*;

pub(crate) fn system_to_vec(system: &DbSystem) -> Vec3 {
    Vec3::new(
        system.position.unwrap().x as f32,
        system.position.unwrap().y as f32,
        system.position.unwrap().z as f32,
    )
}
