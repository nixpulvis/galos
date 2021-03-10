use galos_db::{Error, Database};
use galos_db::systems::System;

#[async_std::main]
async fn main() -> Result<(), Error> {
    let db = Database::new().await?;
    let system = System::fetch_by_name(&db, "Sol").await?;
    println!("SELECT: {:#?}", system);

    Ok(())
}
