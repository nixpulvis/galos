use galos_db::articles::Article;
use galos_db::{Database, Error, Page};
use sqlx::types::chrono::NaiveDate;

#[async_std::main]
async fn main() -> Result<(), Error> {
    let db = Database::new().await?;

    let title = None;
    let date = NaiveDate::from_yo_opt(3307, 42).unwrap();
    let body = "this is a test...".into();
    let article = Article::create(&db, title, date, body).await?;
    println!("INSERT: {:#?}", article);

    // TODO: I think we should have the result be the page book itself, so you can call another
    // fetch like `articles.next_page()` to continue getting all the rows.
    let articles = Article::fetch_all(&db, Page::by(20)).await?;
    println!("SELECT: {:#?}", articles);

    let from = NaiveDate::from_yo_opt(3301, 1).unwrap();
    let to = NaiveDate::from_yo_opt(3302, 1).unwrap();
    let articles = Article::fetch_dates(&db, from, to).await?;
    println!("SELECT DATES: {:#?}", articles);

    article.delete(&db).await?;
    println!("DELETE");

    Ok(())
}
