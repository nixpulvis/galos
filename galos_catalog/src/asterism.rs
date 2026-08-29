//! Constellation figures: the lines a constellation is drawn with.
//!
//! A [`Constellation`](crate::constellation::Constellation) is the sky's agreed
//! *region*; an asterism is the *figure* traced across it — the stick figure a
//! person recognises, the Plough in Ursa Major or the belt-and-shoulders of
//! Orion. Unlike the region, the figure is a drawing convention rather than a
//! measurement: there is no single official set of lines, and different atlases
//! join the stars differently. So it is data, not a table baked into the code.
//!
//! # The seam a renderer draws against
//!
//! What a renderer needs is line segments in space; how those are named, and by
//! what file or survey, is not its concern. [`Figures`] is that seam: it turns a
//! set of stars into the endpoint pairs to draw, and a renderer takes a
//! `&impl Figures` without knowing whether the figures were read from a
//! Stellarium file, keyed by Hipparcos number or by name, or generated. The
//! parser below is one provider of it; another format is another `Figures`, and
//! nothing downstream changes.
//!
//! # The format this module reads
//!
//! Stellarium's `constellationship.fab`, the one most published line sets use:
//! one constellation to a line, the IAU abbreviation, a segment count `N`, then
//! `2N` Hipparcos numbers read in pairs, each pair a line. HIP because that is
//! the identity a figure file and a star catalog can both speak — a
//! [`Star`](crate::Star) carries its [`hip`](crate::Star::hip).
//!
//! ```text
//! # Ursa Major, the seven of the Plough
//! UMa 6 54061 53910 53910 58001 58001 59774 59774 62956 62956 65378 65378 67301
//! ```

use crate::Star;
use crate::constellation::{self, Constellation};
use std::collections::HashMap;
use std::io::{self, BufRead};

/// A source of constellation figures, resolved against a set of stars.
///
/// The interface a renderer draws through, so it is tied to neither the file
/// format nor the identity a figure names its stars by. An implementation joins
/// its own figures to the stars it is handed and yields the line segments to
/// draw, each a pair of endpoint positions in the catalog's frame (light
/// years). A segment whose stars are not among those handed in is dropped
/// rather than reported: a figure is drawn with what is present.
pub trait Figures {
    /// The lines to draw, each a pair of endpoint positions in light years.
    fn segments(&self, stars: &[Star]) -> Vec<[[f64; 3]; 2]>;
}

/// One constellation's figure: the segments its lines are drawn as.
///
/// Each segment is a pair of Hipparcos numbers, the two stars a line joins. The
/// numbers are the figure file's; whether a catalog carries those stars is the
/// join [`Figures`] makes, so a figure naming a star a given catalog lacks
/// simply goes undrawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Asterism {
    /// The IAU abbreviation the figure is for, as [`constellation`] spells it.
    pub abbreviation: String,
    /// The lines, each a pair of Hipparcos numbers to join.
    pub segments: Vec<[u32; 2]>,
}

impl Asterism {
    /// The constellation this figure traces, where its abbreviation is one of
    /// the IAU's; [`None`] for a token the table does not know.
    pub fn constellation(&self) -> Option<&'static Constellation> {
        constellation::from_abbreviation(&self.abbreviation)
    }
}

/// A slice of Hipparcos-keyed figures is a [`Figures`] provider: it resolves
/// each segment's two HIP numbers against the stars handed in.
///
/// Built once into a HIP-to-position map, so resolving a whole figure set is
/// linear in the segments rather than a scan per endpoint. `Vec<Asterism>`
/// derefs to this, so a parsed file is a provider as it stands.
impl Figures for [Asterism] {
    fn segments(&self, stars: &[Star]) -> Vec<[[f64; 3]; 2]> {
        let by_hip: HashMap<u32, [f64; 3]> = stars
            .iter()
            .filter_map(|s| s.hip.map(|h| (h, s.position)))
            .collect();
        let mut lines = Vec::new();
        for figure in self {
            for [a, b] in &figure.segments {
                if let (Some(&from), Some(&to)) = (by_hip.get(a), by_hip.get(b))
                {
                    lines.push([from, to]);
                }
            }
        }
        lines
    }
}

/// An owned figure set is a provider too, so a parsed file passes straight to a
/// renderer without a slice at the call site.
impl Figures for Vec<Asterism> {
    fn segments(&self, stars: &[Star]) -> Vec<[[f64; 3]; 2]> {
        self.as_slice().segments(stars)
    }
}

/// Parse a Stellarium `constellationship.fab` figure file.
///
/// One asterism per non-empty line: an abbreviation, a segment count `N`, then
/// `2N` Hipparcos numbers read in pairs. Blank lines and lines beginning with
/// `#` are skipped, so a file may be commented. A line whose count and numbers
/// do not add up is an error rather than a guess, since a figure half-read
/// draws wrong lines.
pub fn parse<R: io::Read>(reader: R) -> io::Result<Vec<Asterism>> {
    let mut asterisms = Vec::new();
    for line in io::BufReader::new(reader).lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let abbreviation = match tokens.next() {
            Some(abbreviation) => abbreviation.to_owned(),
            None => continue,
        };
        let count: usize = tokens
            .next()
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| invalid(&abbreviation, "a segment count"))?;
        let mut segments = Vec::with_capacity(count);
        for _ in 0..count {
            let a = hip(&mut tokens, &abbreviation)?;
            let b = hip(&mut tokens, &abbreviation)?;
            segments.push([a, b]);
        }
        asterisms.push(Asterism { abbreviation, segments });
    }
    Ok(asterisms)
}

/// The next Hipparcos number, or an error naming the figure that ran short.
fn hip<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
    abbreviation: &str,
) -> io::Result<u32> {
    tokens
        .next()
        .and_then(|t| t.parse().ok())
        .ok_or_else(|| invalid(abbreviation, "a Hipparcos number"))
}

fn invalid(abbreviation: &str, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{abbreviation}: a figure line is missing {what}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A star at `hip`, at a position that encodes it, so a resolved segment
    /// can be checked back to the stars it joined.
    fn star(hip: u32) -> Star {
        Star {
            source: crate::HYG,
            id: hip as u64,
            hip: Some(hip),
            name: None,
            position: [hip as f64, 0.0, 0.0],
            distance: hip as f64,
            apparent_magnitude: 0.0,
            absolute_magnitude: 0.0,
            color_index: None,
            spectral_type: None,
            constellation: None,
        }
    }

    /// The numbers are read in pairs, each pair a line to draw.
    #[test]
    fn a_figure_is_read_as_pairs_of_stars() {
        let figures = parse("UMa 3 1 2 2 3 3 4\n".as_bytes()).unwrap();
        assert_eq!(figures.len(), 1);
        assert_eq!(figures[0].abbreviation, "UMa");
        assert_eq!(figures[0].segments, vec![[1, 2], [2, 3], [3, 4]]);
    }

    /// A figure names the constellation it traces, through the IAU table.
    #[test]
    fn a_figure_names_its_constellation() {
        let figures = parse("Ori 1 100 200".as_bytes()).unwrap();
        assert_eq!(figures[0].constellation().map(|c| c.name), Some("Orion"));
    }

    /// Blank lines and `#` comments are skipped, so a file may be annotated.
    #[test]
    fn blank_and_comment_lines_are_skipped() {
        let figures =
            parse("# the plough\n\nUMa 1 1 2\n\n".as_bytes()).unwrap();
        assert_eq!(figures.len(), 1);
    }

    /// A line whose numbers do not fill its count is an error, not a partial
    /// figure drawn wrong.
    #[test]
    fn a_line_that_runs_short_is_an_error() {
        assert!(parse("UMa 2 1 2 3".as_bytes()).is_err());
        assert!(parse("UMa notanumber".as_bytes()).is_err());
    }

    /// The [`Figures`] join resolves a segment's HIP pair to the two stars'
    /// positions, and drops a segment whose stars are not present.
    #[test]
    fn the_join_resolves_present_stars_and_drops_the_rest() {
        let figures = parse("UMa 2 1 2 2 9\n".as_bytes()).unwrap();
        let stars = [star(1), star(2)];
        let lines = figures.segments(&stars);
        // 1-2 resolves; 2-9 drops, star 9 not among them.
        assert_eq!(lines, vec![[[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]]);
    }
}
