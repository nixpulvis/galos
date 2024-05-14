use galos_db::systems::System;
use galos_db::{Database, Error};

#[async_std::main]
async fn main() -> Result<(), Error> {
    let db = Database::new().await?;
    let system = System::fetch_by_name(&db, "Sol").await?;
    println!("SELECT: {:#?}", system);

    Ok(())
}
