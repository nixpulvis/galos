//! Articles from Galnet News
use sqlx::types::chrono::NaiveDate;

/// ### Schema
///
/// ```
///                            Table "public.articles"
///  Column |  Type   | Collation | Nullable |               Default
/// --------+---------+-----------+----------+--------------------------------------
/// id     | integer |           | not null | nextval('articles_id_seq'::regclass)
/// title  | text    |           |          |
/// date   | date    |           | not null |
/// body   | text    |           | not null |
/// Indexes:
///     "articles_pkey" PRIMARY KEY, btree (id)
///     "body_gist" gist (body gist_trgm_ops)
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct Article {
    pub id: i32,
    pub title: Option<String>,
    pub date: NaiveDate,
    pub body: String,
}

mod create;
mod fetch;
