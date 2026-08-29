//! Render a star catalog to a picture.
//!
//! ```sh
//! # Stand on Sol, look at Sirius.
//! cargo run --release -p galos_sky -- hygdata.csv --at Sirius -o sirius.png
//!
//! # Stand at Sirius and look back at the Sun.
//! cargo run --release -p galos_sky -- hygdata.csv --from Sirius --at Sol -o home.png
//! ```
//!
//! `--from` and `--at` take a star's proper name, or `x,y,z` in light years,
//! or `Sol` for the origin. Everything else is the two dials: `--fov` for how
//! much sky, `--exposure` for the magnitude that fills a pixel.

use clap::Parser;
use galos_catalog::asterism;
use galos_catalog::{Star, hyg};
use galos_photometry::psf::Profile;
use galos_sky::Camera;
use std::fs::File;
use std::path::PathBuf;
use std::process::exit;

/// Draw a star catalog from somewhere in it.
#[derive(Parser)]
#[command(name = "galos-sky", version, about)]
struct Cli {
    /// The HYG catalog CSV to read.
    catalog: PathBuf,

    /// Where to stand: a star's name, `x,y,z` in light years, or `Sol`.
    #[arg(long, default_value = "Sol")]
    from: String,

    /// What to look at, in the same forms as `--from`.
    #[arg(long, default_value = "Sirius")]
    at: String,

    /// Vertical field of view, degrees.
    #[arg(long, default_value_t = 60.0)]
    fov: f64,

    /// The apparent magnitude that fills a pixel. Higher shows fainter stars.
    #[arg(long, default_value_t = 7.5)]
    exposure: f64,

    /// The stellar core width, in arcminutes.
    #[arg(long, default_value_t = galos_sky::camera::DEFAULT_SEEING_ARCMIN)]
    seeing: f64,

    /// The point-spread profile: `moffat` (default) or `gaussian`.
    #[arg(long, default_value = "moffat", value_parser = parse_profile)]
    profile: Profile,

    /// Share of a star's light in its halo, 0..1. Zero draws a bare core.
    #[arg(long, default_value_t = galos_photometry::psf::AUREOLE_WEIGHT)]
    aureole_weight: f64,

    /// How much broader the halo is than the seeing core.
    #[arg(long, default_value_t = galos_photometry::psf::AUREOLE_WIDTH)]
    aureole_width: f64,

    /// The halo's Moffat wing index; smaller reaches further.
    #[arg(long, default_value_t = galos_photometry::psf::AUREOLE_BETA)]
    aureole_beta: f64,

    /// Image size, `WIDTHxHEIGHT`.
    #[arg(long, default_value = "1600x900")]
    size: String,

    /// Ring these stars, by name, comma separated. Green, a colour no star
    /// can be.
    #[arg(long, value_name = "NAMES")]
    highlight: Option<String>,

    /// Ring, in magenta, the stars this picture is missing: the ones the
    /// catalog locates on the sky but not in space. Only meaningful from the
    /// origin, which is where their bearings were measured.
    #[arg(long)]
    show_missing: bool,

    /// Only show missing stars at least this bright.
    #[arg(long, default_value_t = 6.0)]
    missing_limit: f64,

    /// The radius of a highlight ring, pixels.
    #[arg(long, default_value_t = 12.0)]
    highlight_radius: f64,

    /// Draw constellation figures from a Stellarium-format lines file: one
    /// constellation per line, an IAU abbreviation, a segment count, then that
    /// many Hipparcos-number pairs. Lines join stars the catalog carries.
    #[arg(long, value_name = "FILE")]
    constellations: Option<PathBuf>,

    /// Where to write the PNG.
    #[arg(short, long, default_value = "sky.png")]
    output: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    let file = File::open(&cli.catalog).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", cli.catalog.display());
        exit(1);
    });
    let catalog = hyg::read(file).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", cli.catalog.display());
        exit(1);
    });
    let galos_catalog::Catalog { stars, unplaced, skipped, .. } = catalog;
    eprintln!(
        "{} stars, {} skipped ({} with no parallax, {} at the origin, {} unreadable)",
        stars.len(),
        skipped.total(),
        skipped.no_parallax,
        skipped.at_the_origin,
        skipped.unreadable,
    );
    // Said separately and only when it is true, because it is the part that
    // changes a picture: the stars with no parallax are the distant luminous
    // ones, so the hole they leave sits at the bright end rather than the
    // faint one.
    if skipped.naked_eye > 0 {
        eprintln!(
            "warning: {} of the skipped are naked-eye stars, brightest magnitude {:.2}",
            skipped.naked_eye,
            skipped.brightest.unwrap_or(f64::NAN),
        );
    }

    let (width, height) = parse_size(&cli.size);
    let from = place(&stars, &cli.from);
    let at = place(&stars, &cli.at);
    if from == at {
        eprintln!("cannot look at where the camera stands");
        exit(1);
    }

    let camera = Camera::new(width, height)
        .looking_from(from, at)
        .with_fov_degrees(cli.fov)
        .with_exposure(cli.exposure)
        .with_seeing(cli.seeing)
        .with_profile(cli.profile)
        .without_aureoles()
        .with_aureole(galos_sky::camera::Aureole {
            weight: cli.aureole_weight,
            width: cli.aureole_width,
            beta: cli.aureole_beta,
        });

    let image = camera.render(&stars);
    let lit =
        image.pixels().iter().filter(|p| p[0] + p[1] + p[2] > 1e-3).count();
    eprintln!(
        "{}x{}, {lit} pixels lit, peak {:.3}, total {:.1}",
        image.width(),
        image.height(),
        image.peak(),
        image.total_energy(),
    );

    // Ringed after the render, so the figures printed above are the
    // picture's own photometry and not the overlay's.
    let mut marks: Vec<_> = cli
        .highlight
        .iter()
        .flat_map(|names| names.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| {
            let star = stars.iter().find(|s| {
                s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))
            });
            match star {
                Some(star) => camera.mark(star.position, cli.highlight_radius),
                None => {
                    eprintln!("warning: no star named {name} to highlight");
                    None
                }
            }
        })
        .collect();
    if !marks.is_empty() {
        eprintln!("{} highlighted", marks.len());
    }

    if cli.show_missing {
        let candidates: Vec<_> = unplaced
            .iter()
            .filter(|u| u.apparent_magnitude <= cli.missing_limit)
            .collect();
        // Counted by what frames, not by what is in front: over a wide field
        // most of the sky is ahead of the eye and off the picture, and a ring
        // that lands off the picture is not a ring anybody sees.
        let holes: Vec<_> = candidates
            .iter()
            .filter_map(|u| {
                camera.mark_bearing(u.direction, cli.highlight_radius)
            })
            .filter(|m| camera.frames(m))
            .map(|m| m.colored(galos_sky::image::HOLE_COLOR))
            .collect();

        if candidates.is_empty() {
            eprintln!("no missing stars brighter than {}", cli.missing_limit);
        } else if camera.position != [0.0; 3] {
            eprintln!(
                "note: missing stars cannot be drawn from here. A bearing \
                 carries no distance, so it locates a star only from the \
                 origin it was measured from."
            );
        } else {
            eprintln!(
                "{} of {} missing stars brighter than {} are in this frame",
                holes.len(),
                candidates.len(),
                cli.missing_limit,
            );
        }
        marks.extend(holes);
    }

    let segments = cli
        .constellations
        .as_ref()
        .map(|path| {
            let file = File::open(path).unwrap_or_else(|e| {
                eprintln!("cannot open {}: {e}", path.display());
                exit(1);
            });
            let figures = asterism::parse(file).unwrap_or_else(|e| {
                eprintln!("cannot read {}: {e}", path.display());
                exit(1);
            });
            let lines = camera.figure_lines(&stars, &figures);
            eprintln!(
                "{} figure lines from {} constellations",
                lines.len(),
                figures.len(),
            );
            lines
        })
        .unwrap_or_default();

    if let Err(e) = image.write_png_over(&cli.output, &marks, &segments) {
        eprintln!("cannot write {}: {e}", cli.output.display());
        exit(1);
    }
    println!("{}", cli.output.display());
}

/// A place named on the command line: a star, a coordinate triple, or Sol.
fn place(stars: &[Star], name: &str) -> [f64; 3] {
    if name.eq_ignore_ascii_case("sol") {
        return [0.0; 3];
    }
    let parts: Vec<&str> = name.split(',').collect();
    if parts.len() == 3
        && let (Ok(x), Ok(y), Ok(z)) = (
            parts[0].trim().parse(),
            parts[1].trim().parse(),
            parts[2].trim().parse(),
        )
    {
        return [x, y, z];
    }
    match stars.iter().find(|s| {
        s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))
    }) {
        Some(star) => star.position,
        None => {
            eprintln!("no star named {name}, and not an x,y,z triple");
            exit(1);
        }
    }
}

/// `WIDTHxHEIGHT`.
fn parse_size(size: &str) -> (u32, u32) {
    let (w, h) = size.split_once(['x', 'X']).unwrap_or(("1600", "900"));
    (w.trim().parse().unwrap_or(1600), h.trim().parse().unwrap_or(900))
}

/// `moffat` or `gaussian`, for the `--profile` flag.
fn parse_profile(s: &str) -> Result<Profile, String> {
    match s.to_ascii_lowercase().as_str() {
        "moffat" => Ok(Profile::Moffat),
        "gaussian" => Ok(Profile::Gaussian),
        other => Err(format!(
            "unknown profile {other:?}, expected moffat or gaussian"
        )),
    }
}
