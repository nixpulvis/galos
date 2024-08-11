use elite_journal::faction::Faction;
use elite_journal::station::{Service, Station as JournalStation, StationType};
use elite_journal::system::{Coordinate, System as JournalSystem};
use elite_journal::{Allegiance, Government};
use galos_db::stations::Station;
use galos_db::systems::System;
use galos_db::{Database, Error};
use sqlx::types::chrono::Utc;

#[async_std::main]
async fn main() -> Result<(), Error> {
    let db = Database::new().await?;
    let system_address = 0;
    let system = JournalSystem::new(system_address, "The Sun");
    System::from_journal(&db, Utc::now(), &system).await;
    let station = JournalStation {
        dist_from_star_ls: None,
        name: "Maxland".into(),
        ty: Some(StationType::Orbis),
        market_id: Some(1),
        faction: Some(Faction {
            name: "Ours".into(),
            state: None,
        }),
        government: Some(Government::Theocracy),
        allegiance: Some(Allegiance::PlayerPilots),
        services: Some(vec![Service::Contacts]),
        economies: None,
        wanted: None,
    };
    let station = Station::from_journal(&db, Utc::now(), &station, system_address).await?;
    println!("{:#?}", station);

    Ok(())
}
