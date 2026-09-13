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
        let at = self.lines.at() + 1;
        match self.lines.next()? {
            None => Ok(None),
            Some(text) => serde_json::from_str(text)
                .map(Some)
                .map_err(|err| io::Error::other(format!("line {at}: {err}"))),
        }
    }

    /// Which line the reader is on.
    pub fn at(&self) -> u64 {
        self.lines.at()
    }
}
