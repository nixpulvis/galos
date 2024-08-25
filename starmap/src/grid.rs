use bevy::prelude::*;
use bevy_infinite_grid::InfiniteGridBundle;

pub fn spawn(mut commands: Commands) {
    commands.spawn(InfiniteGridBundle::default());
}
