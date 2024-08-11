use std::fmt::Display;
use askama::Template;
use axum::{
    extract,
    routing::get,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Router,
};
use serde::Deserialize;
use galos_db::Database;
use galos_db::systems::System;
use galos_db::stations::Station;

#[tokio::main]
async fn main() {
    // initialize tracing
    tracing_subscriber::fmt::init();

    // build our application with a route
    let app = Router::new()
        .route("/", get(root))
        .route("/systems", get(systems))
        .route("/systems/:address", get(system))
        .route("/systems/:address/stations/:name", get(station));

    // run our app with hyper, listening globally on port 3000
    let addr = "0.0.0.0:3000";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("running the server on {}", addr);
    axum::serve(listener, app).await.unwrap();
}

fn table_data<T: Display>(option: &Option<T>) -> String {
    option.as_ref().map(|o| o.to_string()).unwrap_or("---".into())
}

#[derive(Template)]
#[template(path = "systems.html")]
struct SystemsTemplate {
    query: String,
    systems: Vec<System>,
}

#[derive(Template)]
#[template(path = "system.html")]
struct SystemTemplate {
    system: System,
    stations: Vec<Station>,
}

#[derive(Template)]
#[template(path = "station.html")]
struct StationTemplate {
    system: System,
    station: Station,
}

struct HtmlTemplate<T>(T);

impl<T> IntoResponse for HtmlTemplate<T>
where T: Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template. Error: {err}"),
            ).into_response()
        }
    }
}

// basic handler that responds with a static string
async fn root() -> &'static str {
    "Hello, World!"
}

#[derive(Deserialize)]
struct SystemsParams {
    query: Option<String>,
    // TODO: Advanced search not SQL
}

async fn systems(extract::Query(params): extract::Query<SystemsParams>) -> impl IntoResponse {
    let query = params.query.unwrap_or_default();
    if let Ok(db) = Database::new().await {
        if let Ok(systems) = System::fetch_like_name(&db, &query).await {
            let template = SystemsTemplate { query, systems };
            HtmlTemplate(template).into_response()
        } else {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch systems."),
            ).into_response()
        }
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load DB."),
        ).into_response()
    }
}

async fn system(extract::Path(address): extract::Path<i64>) -> impl IntoResponse {
    if let Ok(db) = Database::new().await {
        if let Ok(system) = System::fetch(&db, address).await {
            let stations = Station::fetch_all(&db, address).await.unwrap_or_default();
            HtmlTemplate(SystemTemplate { system, stations }).into_response()
        } else {
            (
                StatusCode::NOT_FOUND,
                format!("No system with that address found."),
            ).into_response()
        }
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load DB."),
        ).into_response()
    }
}

async fn station(extract::Path((address, name)): extract::Path<(i64, String)>) -> impl IntoResponse {
    if let Ok(db) = Database::new().await {
        if let Ok(station) = Station::fetch(&db, address, &name).await {
            let system = System::fetch(&db, station.system_address).await.unwrap();
            HtmlTemplate(StationTemplate { system, station }).into_response()
        } else {
            (
                StatusCode::NOT_FOUND,
                format!("No station with that address found."),
            ).into_response()
        }
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load DB."),
        ).into_response()
    }
}

// async fn create_user(
//     // this argument tells axum to parse the request body
//     // as JSON into a `CreateUser` type
//     Json(payload): Json<CreateUser>,
// ) -> (StatusCode, Json<User>) {
//     // insert your application logic here
//     let user = User {
//         id: 1337,
//         username: payload.username,
//     };

//     // this will be converted into a JSON response
//     // with a status code of `201 Created`
//     (StatusCode::CREATED, Json(user))
// }
