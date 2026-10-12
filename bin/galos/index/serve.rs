//! `galos index serve`: a build directory over HTTP.
//!
//! The paths and what each answers are `galos_index::read::http`'s, which
//! knows nothing about HTTP; this is the server around it. Every answer goes
//! out with its stamp as an `ETag` and `Cache-Control: no-cache`, so a cache
//! in front of it keeps the bytes and asks again before using them, and a
//! request whose `If-None-Match` still holds is answered `304` with nothing
//! read. A `HEAD` is the stamp alone: what a reader polls with.
//!
//! Open to any origin, since the reader it is for is a page served from
//! somewhere else. Nothing here is private: the directory is what the map
//! draws.

use galos_index::read::http::{self, Answer, Body, Route};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// Serve `dir` at `listen` on `threads` threads until `stopping` says so.
pub(super) fn serve(
    dir: &Path,
    listen: &str,
    threads: usize,
    stopping: &(dyn Fn() -> bool + Sync),
) -> ExitCode {
    if !dir.join(galos_index::codec::layout::INDEX_FILE).exists() {
        eprintln!("{}: no index here to serve", dir.display());
        return ExitCode::FAILURE;
    }
    let server = match Server::http(listen) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("cannot listen on {listen}: {err}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(dir = %dir.display(), %listen, "serving");
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                while !stopping() {
                    match server.recv_timeout(Duration::from_millis(250)) {
                        Ok(Some(request)) => respond(dir, request),
                        Ok(None) => {}
                        Err(err) => tracing::warn!(%err, "a failed accept"),
                    }
                }
            });
        }
    });
    ExitCode::SUCCESS
}

/// Answer one request, and say so where answering it failed.
fn respond(dir: &Path, request: Request) {
    let head = match request.method() {
        Method::Get => false,
        Method::Head => true,
        _ => return reply(request, empty(405)),
    };
    let (path, query) = match request.url().split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (request.url(), None),
    };
    let Some(route) = Route::parse(path, query) else {
        return reply(request, empty(404));
    };
    let held = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("If-None-Match"))
        .and_then(|h| http::stamp_in(h.value.as_str()));

    // Asked whether it has changed: answered off the stamp, with nothing
    // read, whether the request was a `HEAD` or a `GET`.
    let stamp = match http::stamp(dir, &route) {
        Ok(stamp) => stamp,
        Err(err) => return failed(request, &route, err),
    };
    if held.is_some() && held == stamp {
        return reply(request, stamped(empty(304), stamp));
    }
    if head {
        let status = if stamp.is_some() { 200 } else { 404 };
        // A body file carries no stamp and is there or not only when read.
        let status = if let Route::Bodies(_) = route { 200 } else { status };
        return reply(request, stamped(empty(status), stamp));
    }

    let Answer { stamp, body } = match http::answer(dir, &route) {
        Ok(answer) => answer,
        Err(err) => return failed(request, &route, err),
    };
    let response = match body {
        Body::File(file, len) => Response::new(
            StatusCode(200),
            Vec::new(),
            Box::new(file) as Box<dyn std::io::Read + Send>,
            Some(len as usize),
            None,
        ),
        Body::Bytes(bytes) => {
            let len = bytes.len();
            Response::new(
                StatusCode(200),
                Vec::new(),
                Box::new(std::io::Cursor::new(bytes))
                    as Box<dyn std::io::Read + Send>,
                Some(len),
                None,
            )
        }
        Body::Missing => empty(404),
    };
    reply(request, stamped(response, stamp));
}

type Reply = Response<Box<dyn std::io::Read + Send>>;

/// A response with no body.
fn empty(status: u16) -> Reply {
    Response::new(
        StatusCode(status),
        Vec::new(),
        Box::new(std::io::empty()) as Box<dyn std::io::Read + Send>,
        Some(0),
        None,
    )
}

/// `response` with its stamp as an `ETag`, where it has one.
fn stamped(response: Reply, stamp: Option<u64>) -> Reply {
    match stamp {
        Some(stamp) => response.with_header(header("ETag", &http::etag(stamp))),
        None => response,
    }
}

/// Send `response` with the headers every answer carries.
fn reply(request: Request, response: Reply) {
    let response = response
        .with_header(header("Content-Type", "application/octet-stream"))
        .with_header(header("Cache-Control", "no-cache"))
        .with_header(header("Access-Control-Allow-Origin", "*"))
        .with_header(header("Access-Control-Expose-Headers", "ETag"));
    if let Err(err) = request.respond(response) {
        tracing::debug!(%err, "a reader went away mid-answer");
    }
}

/// A `500` for a read that failed, said here as well as there.
fn failed(request: Request, route: &Route, err: std::io::Error) {
    tracing::warn!(url = %route.to_url(), %err, "a failed read");
    reply(request, empty(500));
}

fn header(field: &str, value: &str) -> Header {
    Header::from_bytes(field, value).expect("a header of plain ASCII")
}
