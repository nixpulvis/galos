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
use galos_catalog::{Star, hyg};
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
    #[arg(long, default_value_t = 6.0)]
    exposure: f64,

    /// The point-spread function's width, pixels.
    #[arg(long, default_value_t = 1.8)]
    seeing: f64,

    /// Image size, `WIDTHxHEIGHT`.
    #[arg(long, default_value = "1600x900")]
    size: String,

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
    let (stars, skipped) = hyg::read(file).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", cli.catalog.display());
        exit(1);
    });
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
        .with_seeing(cli.seeing);

    let image = camera.render(&stars);
    let lit = image
        .pixels()
        .iter()
        .filter(|p| p[0] + p[1] + p[2] > 1e-3)
        .count();
    eprintln!(
        "{}x{}, {lit} pixels lit, peak {:.3}, total {:.1}",
        image.width(),
        image.height(),
        image.peak(),
        image.total_energy(),
    );

    if let Err(e) = image.write_png(&cli.output) {
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
    match stars
        .iter()
        .find(|s| s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    {
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
