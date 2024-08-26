use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use crate::{Searched, MoveCamera};
use galos_db::{Database, systems::System};

/// Move the camera to the searched system
///
/// A system for moving the camera to the searched system and letting
/// the `fetch` system's `fetch_around_camera` logic handle the rest.
pub fn system(
    mut search_events: EventReader<Searched>,
    mut camera_events: EventWriter<MoveCamera>,
) {
    for event in search_events.read() {
        match event {
            Searched::System { name, .. } => {
                future::block_on(async {
                    let db = Database::new().await.unwrap();
                    if let Ok(origin) = System::fetch_by_name(&db, &name).await {
                        if let Some(p) = origin.position {
                            let position = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
                            camera_events.send(MoveCamera { position });
                        }
                    }
                });
            },
            _ => {}
        };
    }
}
