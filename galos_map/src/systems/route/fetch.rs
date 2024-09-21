use crate::systems::fetch::{fetch_condition, FetchIndex, FetchTasks, Fetched};
use crate::Db;
use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;
use galos_db::systems::System as DbSystem;

pub fn fetch_route(
    start: String,
    end: String,
    range: String,
    fetched: &mut ResMut<Fetched>,
    tasks: &mut ResMut<FetchTasks>,
    time: &Res<Time<Real>>,
    db: &Res<Db>,
) {
    let index = FetchIndex::Route(start.clone(), end.clone(), range.clone());
    let now = time.last_update().unwrap_or(time.startup());
    if fetch_condition(&index, &fetched, &tasks, now) {
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
        fetched.0.insert(index.clone(), now);
        tasks.fetched.insert(index, task);
    }
}
