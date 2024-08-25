use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use crate::{Searched, MoveCamera};
use galos_db::{Database, systems::System};

pub fn process(
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
            // Searched::Faction { name } =>
            // Searched::Route { start, end, range } =>
            _ => {}
        };
    }
}
