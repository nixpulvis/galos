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
//! The two forms of a system are two modules. [`systems`] is the brief
//! one, which `systems.json` and its dated slices hold; [`galaxy`] is the
//! full one, which `galaxy.json` and its dated slices hold. They share the
//! framing and nothing else: the full form spells the system's update time
//! `date`, carries no `mainStar`, and hangs a body on the system for every
//! star, planet and barycentre anybody has looked at.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek};
use std::path::Path;

/// One system as the full dump gives it, and the scans it stands for.
pub mod galaxy;
/// Translating the dump's prose for a star class into the game's own.
pub mod star;
/// One system as the brief dump gives it.
pub mod systems;

pub use star::class_of;
pub use systems::System;

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

/// Which of the two forms a dump is in.
///
/// They are told apart by the fields that are not in both: the full form
/// spells the system's own time `date` and hangs `bodies` off it, where
/// the brief form has `updateTime` and no bodies at all. A body of the
/// full form carries an `updateTime` of its own, so the tell for the
/// brief form is that field *and* neither of the other two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// `systems.json` and its dated slices: [`System`].
    Brief,
    /// `galaxy.json` and its dated slices: [`galaxy::System`].
    Full,
}

impl Form {
    /// The form an object's text is in, or [`None`] where it says
    /// neither.
    fn of(text: &str) -> Option<Form> {
        match text.contains("\"bodies\":") || text.contains("\"date\":") {
            true => Some(Form::Full),
            false => text.contains("\"updateTime\":").then_some(Form::Brief),
        }
    }

    /// What to say about a line that will not parse as this form.
    ///
    /// The brief reader over a full dump fails on line 1 with `missing
    /// field `updateTime``, which says nothing about which file is in
    /// hand — and these two are named alike, published together and tens
    /// of gigabytes each, so that is the mistake to expect rather than a
    /// corrupt line.
    fn mistaken(self, text: &str) -> Option<&'static str> {
        match (self, Form::of(text)?) {
            (Form::Brief, Form::Full) => {
                Some("this is a full dump; read it with `Galaxy`")
            }
            (Form::Full, Form::Brief) => {
                Some("this is a brief dump; read it with `Systems`")
            }
            _ => None,
        }
    }
}

/// Which form the dump at `path` is in, off its first object, or [`None`]
/// for a file with no object in it at all.
pub fn form(path: &Path) -> io::Result<Option<Form>> {
    Ok(Lines::open(path)?.next()?.and_then(Form::of))
}

/// The next object of `lines` as a `T`, naming the line and the form when
/// it will not parse.
fn next_as<T: serde::de::DeserializeOwned>(
    lines: &mut Lines,
    form: Form,
) -> io::Result<Option<T>> {
    let at = lines.at() + 1;
    let Some(text) = lines.next()? else {
        return Ok(None);
    };
    serde_json::from_str(text).map(Some).map_err(|err| {
        io::Error::other(match form.mistaken(text) {
            Some(hint) => format!("line {at}: {err} — {hint}"),
            None => format!("line {at}: {err}"),
        })
    })
}

/// Every system of a `systems.json`, parsed.
///
/// The brief form: a name, a place, the class of the main star and when the
/// system was last heard about. 34.9 GB rather than 610.4 GB.
pub struct Systems {
    lines: Lines,
}

impl Systems {
    /// Open a `systems.json`.
    pub fn open(path: &Path) -> io::Result<Systems> {
        Ok(Systems { lines: Lines::open(path)? })
    }

    /// The next system, or [`None`] at the end.
    ///
    /// A line that will not parse is an error naming the line it was on, so
    /// the caller decides whether one bad row ends the read.
    pub fn next(&mut self) -> io::Result<Option<System>> {
        next_as(&mut self.lines, Form::Brief)
    }

    /// Which line the reader is on.
    pub fn at(&self) -> u64 {
        self.lines.at()
    }
}

/// Every system of a `galaxy.json`, parsed.
///
/// The full form: the same system with its standing and every body anybody
/// has looked at hung off it. What [`galaxy::System::scans`] turns into the
/// entries the rest of the ecosystem is fed.
pub struct Galaxy {
    lines: Lines,
}

impl Galaxy {
    /// Open a `galaxy.json`.
    pub fn open(path: &Path) -> io::Result<Galaxy> {
        Ok(Galaxy { lines: Lines::open(path)? })
    }

    /// The next system, or [`None`] at the end.
    pub fn next(&mut self) -> io::Result<Option<galaxy::System>> {
        next_as(&mut self.lines, Form::Full)
    }

    /// Which line the reader is on.
    pub fn at(&self) -> u64 {
        self.lines.at()
    }
}

/// The published schemas, vendored, and what the tests ask of them.
///
/// `schema/{systems,galaxy}.schema.json` are Spansh's own, copied from
/// <https://docs.spansh.co.uk/schema/> unedited. They are here because
/// three of this crate's lists — the star classes, the planet classes and
/// the fields each form is told apart by — *are* the schema, and a list
/// transcribed by hand goes stale the day the format moves and says
/// nothing: a class missed drops a whole family of star to the fallback,
/// silently, over the whole galaxy.
///
/// So the tests read the file. Refreshing it from the site is how this
/// crate finds out the format moved, and a refresh that adds a value
/// fails a test rather than passing one.
///
/// Not used at run time. Validating 610 GB against a schema would cost
/// more than the parse it duplicates, and serde already refuses what it
/// cannot read.
#[cfg(test)]
pub(crate) mod schema {
    use serde_json::Value;

    /// `systems.json`'s schema, the brief form's.
    pub fn systems() -> Value {
        read(include_str!("../schema/systems.schema.json"))
    }

    /// `galaxy.json`'s schema, the full form's.
    pub fn galaxy() -> Value {
        read(include_str!("../schema/galaxy.schema.json"))
    }

    /// The one object the array holds, which is where every schema here
    /// says anything at all.
    fn read(text: &str) -> Value {
        let schema: Value = serde_json::from_str(text).expect("the schema");
        schema["items"].clone()
    }

    /// What a form's schema says an object cannot be without.
    pub fn required(form: &Value) -> Vec<String> {
        strings(&form["required"])
    }

    /// Every value `mainStar` may take in the brief form: the prose for a
    /// star, and for the planet or barycentre a ship can arrive at
    /// instead.
    pub fn main_stars() -> Vec<String> {
        strings(&systems()["properties"]["mainStar"]["enum"])
    }

    /// Every value a body's `subType` may take in the full form, by the
    /// arm of the `anyOf` it belongs to — `"Planet"` or `"Star"`, as the
    /// schema's own descriptions spell them.
    ///
    /// The two arms are shorter than [`main_stars`]: the full form lists
    /// 43 stars where the brief form's `mainStar` lists the same classes
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

    /// A schema array of strings, as strings.
    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .expect("a list")
            .iter()
            .map(|it| it.as_str().expect("a string").to_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fields a dump is told apart by are the fields its schema says
    /// it cannot be without.
    ///
    /// [`Form::of`] rests on exactly this: were `bodies` to become
    /// optional, or the brief form to grow a `date`, the sniff would start
    /// answering the wrong form for a whole file and nothing else here
    /// would notice.
    #[test]
    fn each_form_requires_what_it_is_told_apart_by() {
        let brief = schema::required(&schema::systems());
        let full = schema::required(&schema::galaxy());

        assert!(brief.contains(&"updateTime".to_string()), "{brief:?}");
        assert!(!brief.contains(&"date".to_string()), "{brief:?}");
        assert!(!brief.contains(&"bodies".to_string()), "{brief:?}");

        assert!(full.contains(&"date".to_string()), "{full:?}");
        assert!(full.contains(&"bodies".to_string()), "{full:?}");
    }

    /// And an object of either form reads as the form it is.
    #[test]
    fn an_object_says_which_form_it_is() {
        let brief = r#"{"id64":1,"name":"N","coords":{"x":0,"y":0,"z":0},"updateTime":"2020-01-01T00:00:00Z"}"#;
        let full = r#"{"id64":1,"name":"N","coords":{"x":0,"y":0,"z":0},"date":"2020-01-01T00:00:00Z","bodies":[{"bodyId":1,"name":"N A","updateTime":"2020-01-01T00:00:00Z"}]}"#;

        assert_eq!(Form::of(brief), Some(Form::Brief));
        assert_eq!(Form::of(full), Some(Form::Full));
        // A body of the full form carries an `updateTime` of its own, so
        // that field alone cannot be the brief form's tell.
        assert!(full.contains("\"updateTime\""));
        assert_eq!(Form::of(r#"{"id64":1}"#), None);
    }
}
