//! Articles from Galnet News
use sqlx::types::chrono::NaiveDate;

#[derive(Debug, PartialEq, Eq)]
pub struct Article {
    pub id: i32,
    pub title: Option<String>,
    pub date: NaiveDate,
    pub body: String,
}

mod create;
mod fetch;
