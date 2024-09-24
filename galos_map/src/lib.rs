//! A 3D Galaxy Map for `galos`
//!
//! ![](https://github.com/nixpulvis/galos/blob/master/galos_map/demo.gif?raw=true)
//!
//! Requires (read-only) access to [`galos_db`].
use bevy::prelude::*;
use galos_db::Database;

pub mod camera;
pub mod search;
pub mod systems;
pub mod ui;

#[derive(Resource)]
pub struct Db(pub Database);
