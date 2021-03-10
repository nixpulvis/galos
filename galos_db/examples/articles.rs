use sqlx::types::chrono::NaiveDate;
use galos_db::{Error, Database, Page};
use galos_db::articles::Article;

#[async_std::main]
async fn main() -> Result<(), Error> {
    let db = Database::new().await?;

    let title = None;
    let date = NaiveDate::from_yo(3307, 42);
    let body = "this is a test...".into();
    let article = Article::create(&db, title, date, body).await?;
    println!("INSERT: {:#?}", article);

    // TODO: I think we should have the result be the page book itself, so you can call another
    // fetch like `articles.next_page()` to continue getting all the rows.
    let articles = Article::fetch_all(&db, Page::by(20)).await?;
    println!("SELECT: {:#?}", articles);

    let from = NaiveDate::from_yo(3301, 1);
    let to = NaiveDate::from_yo(3302, 1);
    let articles = Article::fetch_dates(&db, from, to).await?;
    println!("SELECT DATES: {:#?}", articles);

    article.delete(&db).await?;
    println!("DELETE");

    Ok(())
}
