use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use crate::{Db, MoveCamera};
use galos_db::systems::System;

/// A collection of search events for responding to the user's UI
/// interactions.
#[derive(Event, Debug)]
pub enum Searched {
    System {
        name: String,
    },
    Faction {
        name: String,
    },
    Route {
        start: String,
        end: String,
        range: String,
    },
}

/// Move the camera to the searched system
///
/// A system for moving the camera to the searched system and letting
/// the `fetch` system's `fetch_around_camera` logic handle the rest.
pub fn system(
    mut search_events: EventReader<Searched>,
    mut camera_events: EventWriter<MoveCamera>,
    db: Res<Db>,
) {
    for event in search_events.read() {
        match event {
            Searched::System { name, .. } => {
                future::block_on(async {
                    if let Ok(origin) = System::fetch_by_name(&db.0, &name).await {
                        if let Some(p) = origin.position {
                            let position = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
                            camera_events.send(MoveCamera { position: Some(position) });
                        }
                    }
                });
            },
            _ => {}
        };
    }
}
