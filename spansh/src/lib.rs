//! Spansh's galaxy dumps, read a line at a time.
//!
//! <https://spansh.co.uk/dumps> publishes the galaxy as JSON: `systems.json`
//! is every system's name and place, `galaxy.json` is the same with every
//! body and station in it. On the files this was written against:
//!
//! | file | bytes | lines | a line |
//! |---|---|---|---|
//! | `systems.json` | 34.9 GB | 200,071,631 | ~160 B |
//! | `galaxy.json` | 610.4 GB | ~200 M | ~2.8 KB mean, 5.9 MB seen |
//! | `galaxy_7days.json` | 19.9 GB | ~840 K | ~23.7 KB mean |
//!
//! All of them are a JSON array holding one whole object per line — `[`
//! alone on the first line, `]` alone on the last, the objects
//! comma-terminated in between — as the schema says outright. So the
//! framing is a `read_line` and the parsing is `serde_json`, per object,
//! and there is no incremental parser over the array itself. The buffer is
//! reused and grows to the longest line seen.
//!
//! Were a dump ever to arrive as one long line, [`Lines`] would read the
//! whole file into memory.
//!
//! **One system, however much of it a given file carries.** Spansh
//! publishes the same object twice over: `systems.json` states a name, a
//! place and the prose for the star at the middle, and `galaxy.json`
//! states that with the system's standing and every body anybody has
//! looked at hung off it. The schema says so itself — everything but the
//! address, the name, the place and the time is optional — so the reader
//! does not ask which file it was handed and the caller does not either.
//! The one difference that is not an absence is the spelling of the
//! system's own time, `updateTime` in the brief file and `date` in the
//! full one, which is one serde alias.
//!
//! What a system says it does not know reads as [`None`], the same
//! whether the file never carries it or this copy has not been scanned:
//! [`System::class`] answers off the prose where there is prose and off
//! the arrival star where there are bodies, and [`System::scans`] is
//! empty where there are none.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek};
use std::path::Path;

/// Translating the dump's prose for a star class into the game's own.
pub mod star;
/// One system as a dump gives it, and the scans it stands for.
pub mod system;

pub use star::class_of;
pub use system::{Body, Kind, System};

/// How much of the file to hold while a line is being found.
///
/// Large because these files are read start to finish and never seeked: a
/// megabyte a read keeps the syscall count down over 610 GB.
const BUFFER: usize = 1 << 20;

/// The lines of a dump, without the array's own punctuation.
///
/// Yields each object's text, the trailing comma trimmed, ready to be
/// parsed. The opening `[` and closing `]` are skipped, as is a blank
/// line, so what comes out is only ever a candidate object. The buffer is
/// reused across lines.
pub struct Lines {
    reader: BufReader<File>,
    line: String,
    at: u64,
    bytes: u64,
}

impl Lines {
    /// Open a dump.
    pub fn open(path: &Path) -> io::Result<Lines> {
        Lines::open_at(path, 0, 0)
    }

    /// Open a dump at a byte already read up to, on the line already read
    /// up to.
    ///
    /// `at` is a figure [`bytes`](Self::bytes) answered, which is only ever
    /// the end of a line — every line is read whole or not at all — so the
    /// read carries on at the start of the next object and never inside
    /// one. A build that stopped part way through a 610 GB file takes up
    /// where it left off with this.
    pub fn open_at(path: &Path, at: u64, line: u64) -> io::Result<Lines> {
        let mut file = File::open(path)?;
        if at > 0 {
            file.seek(io::SeekFrom::Start(at))?;
        }
        Ok(Lines {
            reader: BufReader::with_capacity(BUFFER, file),
            line: String::new(),
            at: line,
            bytes: at,
        })
    }

    /// Which line the reader is on, for saying where a bad one was.
    pub fn at(&self) -> u64 {
        self.at
    }

    /// How much of the file has been read, in bytes.
    ///
    /// Always a line boundary, so it is what [`open_at`](Self::open_at)
    /// takes to carry on from. Bytes rather than lines because seeking to
    /// a line means counting them.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The next object's text, or [`None`] at the end of the file.
    ///
    /// Borrowed from the reader's own buffer rather than returned by value,
    /// the caller parsing it and dropping it.
    pub fn next(&mut self) -> io::Result<Option<&str>> {
        loop {
            self.line.clear();
            self.at += 1;
            match self.reader.read_line(&mut self.line)? {
                0 => return Ok(None),
                read => self.bytes += read as u64,
            }
            // The object's bounds within the line, so what is returned is a
            // slice of the buffer.
            let object = {
                let text = self.line.trim_end().trim_end_matches(',');
                let start = text.len() - text.trim_start().len();
                (start, text.len())
            };
            match self.line[object.0..object.1].starts_with('{') {
                true => return Ok(Some(&self.line[object.0..object.1])),
                // `[`, `]`, or the blank line a file can end on.
                false => continue,
            }
        }
    }
}

/// Every system of a dump, parsed — whichever dump it is.
///
/// `systems.json`, `galaxy.json` and their dated slices all read through
/// this, because they are one object with more or less of it filled in.
/// What a brief file does not carry comes back as [`None`] and an empty
/// [`System::bodies`].
pub struct Dump {
    lines: Lines,
}

impl Dump {
    /// Open a dump.
    pub fn open(path: &Path) -> io::Result<Dump> {
        Ok(Dump { lines: Lines::open(path)? })
    }

    /// The next system, or [`None`] at the end.
    ///
    /// A line that will not parse is an error naming the line it was on,
    /// so the caller decides whether one bad row ends the read.
    pub fn next(&mut self) -> io::Result<Option<System>> {
        let at = self.lines.at() + 1;
        let Some(text) = self.lines.next()? else {
            return Ok(None);
        };
        serde_json::from_str(text)
            .map(Some)
            .map_err(|err| io::Error::other(format!("line {at}: {err}")))
    }

    /// Which line the reader is on.
    pub fn at(&self) -> u64 {
        self.lines.at()
    }

    /// How much of the file has been read, in bytes, which is always a
    /// line boundary. What [`Lines::open_at`] takes to carry on from.
    pub fn bytes(&self) -> u64 {
        self.lines.bytes()
    }
}

/// The published schemas, vendored, and what the tests ask of them.
///
/// `schema/{systems,galaxy}.schema.json` are Spansh's own, copied from
/// <https://docs.spansh.co.uk/schema/> unedited. They are here because
/// two of this crate's lists — the star classes and the planet classes —
/// *are* the schema, and a list transcribed by hand goes stale the day
/// the format moves and says nothing: a class missed drops a whole
/// family of star to the fallback, silently, over the whole galaxy. The
/// third thing they pin is that [`System`] is enough for both files: its
/// only required fields are the ones both schemas require.
///
/// So the tests read the file. Refreshing it from the site is how this
/// crate finds out the format moved, and a refresh that adds a value
/// fails a test rather than passing one.
///
/// Not used at run time, and it could not be: thirteen of the galaxy
/// schema's nodes are `$ref`s into an OpenAPI document — `#/paths/~1
/// %7Bfilename%7D/get/responses/...` — that is not in the file, so
/// `secondaryEconomy`, `timestamps`, a station, `materials`,
/// `solidComposition`, `atmosphereComposition` and `belts` resolve to
/// nothing here. What the tests read is the inline part, and a node that
/// turned into a `$ref` would fail them rather than quietly answer an
/// empty list. Validating 610 GB against a schema would cost more than
/// the parse it duplicates anyway, and serde already refuses what it
/// cannot read.
#[cfg(test)]
pub(crate) mod schema {
    use serde_json::Value;

    /// `systems.json`'s schema, the brief file's.
    pub fn systems() -> Value {
        read(include_str!("../schema/systems.schema.json"))
    }

    /// `galaxy.json`'s schema, the full file's.
    pub fn galaxy() -> Value {
        read(include_str!("../schema/galaxy.schema.json"))
    }

    /// The one object the array holds, which is where every schema here
    /// says anything at all.
    fn read(text: &str) -> Value {
        let schema: Value = serde_json::from_str(text).expect("the schema");
        schema["items"].clone()
    }

    /// What a file's schema says an object cannot be without.
    pub fn required(of: &Value) -> Vec<String> {
        strings(&of["required"])
    }

    /// Every value `mainStar` may take in the brief file: the prose for
    /// a star, and for the planet or barycentre a ship can arrive at
    /// instead.
    pub fn main_stars() -> Vec<String> {
        strings(&systems()["properties"]["mainStar"]["enum"])
    }

    /// Every value a body's `subType` may take in the full file, by the
    /// arm of the `anyOf` it belongs to — `"Planet"` or `"Star"`, as the
    /// schema's own descriptions spell them.
    ///
    /// The two arms are shorter than [`main_stars`]: the full file lists
    /// 43 stars where the brief file's `mainStar` lists the same classes
    /// and the eighteen planets together.
    pub fn sub_types(kind: &str) -> Vec<String> {
        let body = galaxy()["properties"]["bodies"]["items"].clone();
        let arms = body["properties"]["subType"]["anyOf"]
            .as_array()
            .expect("subType is an anyOf")
            .clone();
        let arm = arms
            .iter()
            .find(|arm| {
                arm["description"]
                    .as_str()
                    .is_some_and(|it| it.ends_with(&format!("of type {kind}.")))
            })
            .unwrap_or_else(|| panic!("no {kind} arm in the schema"));
        strings(&arm["enum"])
    }

    /// Every value a system-level field may take, by its name in the
    /// schema: `allegiance`, `government`, `primaryEconomy`, `security`.
    ///
    /// `null` is a value of three of them and is dropped here, being the
    /// absence the reader already answers with [`None`].
    pub fn system_enum(field: &str) -> Vec<String> {
        strings(&galaxy()["properties"][field]["enum"])
    }

    /// Every value a body-level field may take, by its name in the
    /// schema: `atmosphereType`, `volcanismType`, `type`, and the rest.
    pub fn body_enum(field: &str) -> Vec<String> {
        let body = galaxy()["properties"]["bodies"]["items"].clone();
        strings(&body["properties"][field]["enum"])
    }

    /// A schema array of strings, as strings, `null` left out.
    ///
    /// A node that is a `$ref` is [`Value::Null`] here, the file not
    /// carrying what it points at, and that panics rather than reading
    /// as a list with nothing in it — a test over an empty list is a
    /// test that asks nothing.
    fn strings(value: &Value) -> Vec<String> {
        let list = value.as_array().expect("an inline list, not a $ref");
        assert!(!list.is_empty(), "an empty list asks nothing");
        list.iter().filter_map(|it| it.as_str()).map(str::to_owned).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One [`System`] is enough for both files, and the schemas are what
    /// says so: what either requires, this type requires, and everything
    /// else it may leave out.
    ///
    /// The whole of the difference is two spellings of one field. Were
    /// the brief file to start requiring something the full one does not,
    /// or either to require a field this reads as optional, a line would
    /// arrive with nowhere to put it and only this would notice.
    #[test]
    fn one_system_is_enough_for_either_file() {
        let brief = schema::required(&schema::systems());
        let full = schema::required(&schema::galaxy());

        // What both demand, which is what this type demands.
        for shared in ["id64", "name", "coords"] {
            assert!(brief.contains(&shared.to_string()), "{brief:?}");
            assert!(full.contains(&shared.to_string()), "{full:?}");
        }
        // And the one field spelled two ways, each required of its own
        // file and of neither the other.
        assert!(brief.contains(&"updateTime".to_string()), "{brief:?}");
        assert!(full.contains(&"date".to_string()), "{full:?}");
        assert!(!brief.contains(&"date".to_string()), "{brief:?}");
        assert!(!full.contains(&"updateTime".to_string()), "{full:?}");

        // Nothing else is required of either file, so nothing else may be
        // anything but optional here.
        let optional = |it: &String| {
            !["id64", "name", "coords", "updateTime", "date"]
                .contains(&it.as_str())
        };
        assert_eq!(brief.iter().filter(|it| optional(it)).count(), 0);
        assert_eq!(
            full.iter().filter(|it| optional(it)).collect::<Vec<_>>(),
            vec!["bodies"],
            "the full file requires something this reads as optional",
        );
    }

    /// The least either file can say still reads, and reads the same.
    #[test]
    fn the_least_either_file_can_say_reads() {
        let brief = r#"{"id64":1,"name":"N","coords":{"x":1,"y":2,"z":3},"updateTime":"2020-01-01T00:00:00Z"}"#;
        let full = r#"{"id64":1,"name":"N","coords":{"x":1,"y":2,"z":3},"date":"2020-01-01T00:00:00Z","bodies":[]}"#;

        let brief: System = serde_json::from_str(brief).expect("the brief");
        let full: System = serde_json::from_str(full).expect("the full");

        assert_eq!(brief.update_time, full.update_time);
        assert_eq!(brief.coords, full.coords);
        assert!(brief.bodies.is_empty() && full.bodies.is_empty());
        assert_eq!(brief.class(), None);
        assert_eq!(full.class(), None);
    }

    /// Every value the schema's closed lists hold is a value this can
    /// read, and a value refused is a system lost whole.
    ///
    /// A system whose government the reader will not parse is a line
    /// that fails, so its bodies, its place and its name go with it —
    /// which is how the anarchy security reading cost 13% of a
    /// seven-day file before anybody noticed. This is that check made
    /// ahead of the loss, over every field of the system the schema
    /// closes.
    ///
    /// It found four the day it was written: `Frontline Solutions`,
    /// `Private Ownership`, `Private Enterprise` and `Repair`, none of
    /// them in the seven-day file and all of them published as things a
    /// system may be.
    #[test]
    fn every_value_a_system_may_have_reads() {
        for field in ["allegiance", "government", "primaryEconomy", "security"]
        {
            for value in schema::system_enum(field) {
                let line = format!(
                    r#"{{"id64":1,"name":"N","coords":{{"x":0,"y":0,"z":0}},
                       "date":"2020-01-01T00:00:00Z","{field}":"{value}"}}"#,
                );
                let read: Result<System, _> = serde_json::from_str(&line);
                assert!(read.is_ok(), "{field} = {value:?}: {read:?}");
            }
        }
    }

    /// And every value a *body* may have, over the two the dump states
    /// in prose and this translates rather than stores.
    ///
    /// A body refused is not a system lost — [`System::scans`] leaves
    /// out what it cannot read — but it is a planet's whole atmosphere
    /// or its volcanism gone, on every body of that kind in the galaxy.
    #[test]
    fn every_prose_a_body_may_carry_is_translated() {
        for value in schema::body_enum("atmosphereType") {
            let read = system::atmosphere_of(Some(&value));
            assert!(
                !format!("{read:?}").contains("Unknown"),
                "atmosphere {value:?} read as {read:?}",
            );
        }
        for value in schema::body_enum("volcanismType") {
            let read = system::volcanism_of(&value);
            assert!(
                value == "No volcanism" || read.is_some(),
                "volcanism {value:?} read as nothing",
            );
        }
        // The three kinds a body may be, which decide what a scan is.
        assert_eq!(schema::body_enum("type").len(), 3);
        for value in schema::body_enum("type") {
            let read: Kind =
                serde_json::from_str(&format!("\"{value}\"")).expect(&value);
            assert!(!matches!(read, Kind::Unknown(_)), "{value} is unknown");
        }
    }
}
