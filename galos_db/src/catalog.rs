//! Where a star catalog and the Elite dataset disagree.
//!
//! One place, one job: pull the systems Elite holds for a set of names and hand
//! them to [`galos_catalog::compare`], which does the arithmetic and knows
//! nothing about Postgres. The split is the same one everywhere else in this
//! crate — the database lives here and the meaning lives in the crate that owns
//! it.
//!
//! # What the answer is for
//!
//! Elite's sky is a snapshot of whatever catalogs its forge was seeded from, and
//! it cannot be otherwise: a system's position is its identity, since the id64
//! encodes the boxel it sits in, so revising a star's distance would change the
//! address every bookmark, exploration record and row in this database refers to
//! it by. A re-import would not be an update, it would be a new galaxy wearing
//! the old one's names.
//!
//! Which snapshot is then an empirical question, and this is how it is asked.
//! The stars to watch are the ones where Hipparcos and the modern values
//! disagree loudly — the Pleiades above all, where Hipparcos said about 118
//! parsecs against a ground-based consensus near 136 and Gaia settled it at
//! 136. Whichever Elite sits nearer dates the import on its own.
//!
//! # Matching
//!
//! By name, because that is the only key the two datasets share: Elite has no
//! Hipparcos numbers and the catalog has no system addresses. Names are matched
//! loosely for case and spacing and not at all otherwise, so a star Elite spells
//! differently comes back unmatched rather than paired with the wrong row. A
//! wrong match would be indistinguishable in the report from a real
//! disagreement, which is the one failure this must not have.
//!
//! That is a narrow key. Of HYG's 109,400 usable stars only 460 carry a proper
//! name, and those are the only ones asked about. The catalog also holds Bayer
//! and Flamsteed designations for thousands more, but it writes them as
//! `9Alp CMa` where Elite would say something else entirely, and closing that
//! gap means a table of spelling rules — every one of which is a chance to pair
//! two different stars and report the result as a disagreement. Four hundred
//! and sixty is plenty to fit a frame from and plenty to date an import with,
//! so the narrow key is the right one until something needs more.

use crate::{Database, Result};
use galos_catalog::compare::{Comparison, Reference, compare};
use galos_catalog::Star;
use sqlx::Row;

/// Look up every named catalog star in the Elite dataset and compare them.
///
/// Only the named stars are asked about — a catalog id means nothing to Elite —
/// and only positioned systems come back. The query is a single `IN` over the
/// names, since a few hundred proper names is a small list and the alternative
/// is a round trip apiece.
pub async fn compare_to_catalog(
    db: &Database,
    catalog: &[Star],
) -> Result<Comparison> {
    let names: Vec<String> = catalog
        .iter()
        .filter_map(|s| s.name.clone())
        .collect();

    let rows = sqlx::query(
        "SELECT name, \
                ST_X(position) AS x, ST_Y(position) AS y, ST_Z(position) AS z \
         FROM systems \
         WHERE position IS NOT NULL AND upper(name) = ANY($1)",
    )
    .bind(names.iter().map(|n| n.to_uppercase()).collect::<Vec<_>>())
    .fetch_all(&db.pool)
    .await?;

    let reference: Vec<Reference> = rows
        .iter()
        .map(|row| {
            Ok(Reference {
                name: row.try_get("name")?,
                position: [
                    row.try_get("x")?,
                    row.try_get("y")?,
                    row.try_get("z")?,
                ],
            })
        })
        .collect::<Result<_>>()?;

    Ok(compare(catalog, &reference))
}

/// Render a comparison as a report.
///
/// Kept beside the query rather than in the binary because it is where the
/// judgement about what matters lives: the distances first, because they need
/// no frame and are what actually differs between two surveys, then the frame
/// itself, then the worst rows by name so a reader can go and look them up.
pub fn report(comparison: &Comparison) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let matched = comparison.matches.len();
    let _ = writeln!(
        out,
        "{matched} stars in both datasets, {} in the catalog alone",
        comparison.unmatched.len(),
    );
    if matched == 0 {
        let _ = writeln!(
            out,
            "\nNothing matched. The two datasets share only names, so either \
             this database holds no systems named after real stars or it \
             spells them differently."
        );
        return out;
    }

    if let Some(median) = comparison.median_distance_error() {
        let _ = writeln!(
            out,
            "median distance disagreement: {:.2}%",
            median * 100.0
        );
    }

    match &comparison.frame {
        Some(frame) => {
            let _ = writeln!(
                out,
                "\nframe fitted from {} stars, median bearing error {:.4}°{}",
                frame.from,
                frame.median_error,
                if frame.is_mirrored() {
                    " (mirrored: the two use opposite handedness)"
                } else {
                    ""
                },
            );
            for row in frame.rotation {
                let _ = writeln!(
                    out,
                    "  [{:>9.6} {:>9.6} {:>9.6}]",
                    row[0], row[1], row[2]
                );
            }
            // A fit that did not converge on anything is worth saying so
            // plainly, since every bearing error below is measured against it.
            if frame.median_error > 1.0 {
                let _ = writeln!(
                    out,
                    "  the fit is poor; the two datasets may not be related by \
                     a single rotation, or the names are matching the wrong \
                     stars"
                );
            }
        }
        None => {
            let _ = writeln!(
                out,
                "\ntoo few matches to fit a frame; distances below still stand, \
                 being invariant under one"
            );
        }
    }

    let _ = writeln!(out, "\nworst disagreements:");
    let _ = writeln!(
        out,
        "  {:<28} {:>10} {:>10} {:>9} {:>9}",
        "star", "catalog", "elite", "diff", "bearing"
    );
    for m in comparison.matches.iter().take(25) {
        let _ = writeln!(
            out,
            "  {:<28} {:>9.1} {:>9.1} {:>8.1}% {:>8}",
            m.name,
            m.catalog_distance,
            m.reference_distance,
            m.distance_error_fraction() * 100.0,
            match m.bearing_error {
                Some(e) => format!("{e:.3}°"),
                None => "-".into(),
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_catalog::compare::Frame;

    fn comparison(matches: Vec<galos_catalog::compare::Match>) -> Comparison {
        Comparison { matches, unmatched: vec![], frame: None }
    }

    fn one(name: &str, catalog: f64, reference: f64) -> galos_catalog::compare::Match {
        galos_catalog::compare::Match {
            name: name.into(),
            catalog_distance: catalog,
            reference_distance: reference,
            bearing_error: Some(0.01),
        }
    }

    /// Nothing matching is said plainly rather than shown as a table of
    /// nothing, because the likeliest cause is a spelling difference and a
    /// reader needs to be told that rather than left to infer it.
    #[test]
    fn an_empty_comparison_explains_itself() {
        let text = report(&comparison(vec![]));
        assert!(text.contains("Nothing matched"));
        assert!(!text.contains("worst disagreements"));
    }

    /// The report carries the numbers a reader came for.
    #[test]
    fn the_report_names_the_worst_rows() {
        let text = report(&comparison(vec![
            one("Pleione", 118.0, 136.0),
            one("Sirius", 8.6, 8.6),
        ]));
        assert!(text.contains("Pleione"), "{text}");
        assert!(text.contains("2 stars in both"), "{text}");
        assert!(text.contains("15.3%"), "{text}");
    }

    /// A mirrored frame is called out rather than left in the matrix for
    /// somebody to notice, since Elite's frame is left-handed and this is the
    /// case the tool will actually meet.
    #[test]
    fn a_mirrored_frame_is_said_out_loud() {
        let mut c = comparison(vec![one("Sirius", 8.6, 8.6)]);
        c.frame = Some(Frame {
            rotation: [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            determinant: -1.0,
            median_error: 0.002,
            from: 40,
        });
        assert!(report(&c).contains("mirrored"));
    }

    /// And a fit that did not work is flagged, because every bearing error in
    /// the table is measured against it.
    #[test]
    fn a_poor_fit_is_flagged() {
        let mut c = comparison(vec![one("Sirius", 8.6, 8.6)]);
        c.frame = Some(Frame {
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            determinant: 1.0,
            median_error: 37.0,
            from: 40,
        });
        assert!(report(&c).contains("the fit is poor"));
    }
}
