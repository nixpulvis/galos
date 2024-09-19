use crate::systems::{FetchIndex, FetchTasks, Fetched};
use crate::Db;
use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;
use galos_db::systems::System as DbSystem;

pub(crate) fn fetch_route(
    start: String,
    end: String,
    range: String,
    fetched: &mut ResMut<Fetched>,
    tasks: &mut ResMut<FetchTasks>,
    db: &Res<Db>,
) {
    let index = FetchIndex::Route(start.clone(), end.clone(), range.clone());
    if !tasks.fetched.contains_key(&index) {
        let task_pool = AsyncComputeTaskPool::get();
        let db = db.0.clone();
        let task = task_pool.spawn(async move {
            if let (Ok(a), Ok(b), Ok(r)) = (
                DbSystem::fetch_by_name(&db, &start).await,
                DbSystem::fetch_by_name(&db, &end).await,
                range.parse::<f64>(),
            ) {
                if let Some(route) = a.route_to(&db, &b, r) {
                    return route.0;
                }
            }
            vec![]
        });
        fetched.0.insert(index.clone());
        tasks.fetched.insert(index, task);
    }
}
