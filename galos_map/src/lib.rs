use bevy::prelude::*;
use galos_db::Database;

pub mod camera;
pub mod search;
pub mod systems;
pub mod ui;

#[derive(Resource)]
pub struct Db(pub Database);
