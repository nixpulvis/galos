//! Golden images: a picture checked in, and the same picture drawn again.
//!
//! The cheapest rung of the ladder in `docs/sky.md`, and the only one that
//! needs nothing but this crate. A deterministic CPU render of a known sky is
//! compared against a PNG on disk, so any change to the projection, the
//! point-spread function, the exposure law, the blackbody colour or the tone
//! curve shows up as a picture that no longer matches — including changes made
//! in `galos_photometry`, which is most of what is being guarded here.
//!
//! The subject is the Big Dipper, because a golden image whose subject is
//! recognisable is one a person can also judge by eye when it fails. Seven
//! stars in a shape everybody knows: if the file has to be regenerated, the
//! new one can be looked at rather than merely accepted.
//!
//! # Why pixels rather than bytes
//!
//! The comparison decodes both sides and compares channels with a tolerance of
//! one part in 255, rather than diffing the PNG bytes. Byte equality would also
//! be asserting the compression settings of whatever `png` release is in the
//! lockfile, and would break on a transcendental function landing one unit in
//! the last place differently on another architecture — neither of which is a
//! regression in anything this crate does.
//!
//! # Regenerating
//!
//! Set `GALOS_SKY_BLESS=1` to overwrite the golden with what the code draws
//! now. Look at the result before committing it; that is the whole point of
//! the subject being a constellation.

use galos_catalog::{Star, hyg};
use galos_sky::Camera;
use std::path::{Path, PathBuf};

/// The stars: the 250 brightest named, cut from the real HYG catalog.
const FIXTURE: &str = include_str!("../../galos_catalog/data/bright.csv");

/// The seven of the Dipper, in the order the asterism is drawn.
const DIPPER: [&str; 7] =
    ["Dubhe", "Merak", "Phecda", "Megrez", "Alioth", "Mizar", "Alkaid"];

fn stars() -> Vec<Star> {
    hyg::read(FIXTURE.as_bytes()).expect("the fixture is a HYG catalog").stars
}

fn named(stars: &[Star], name: &str) -> Star {
    stars
        .iter()
        .find(|s| s.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("{name} should be in the fixture"))
        .clone()
}

/// The mean direction of the seven, a thousand light years out.
///
/// A direction rather than a centroid of positions: the seven lie between 80
/// and 123 light years away, so their positions average to a point well off
/// the axis the asterism actually sits on.
fn dipper_aim(stars: &[Star]) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for name in DIPPER {
        let star = named(stars, name);
        let d = star.distance;
        for i in 0..3 {
            sum[i] += star.position[i] / d;
        }
    }
    let length =
        (sum.iter().map(|c| c * c).sum::<f64>()).sqrt();
    [
        sum[0] / length * 1000.0,
        sum[1] / length * 1000.0,
        sum[2] / length * 1000.0,
    ]
}

/// The camera the golden is drawn with. The settings that read well: a field
/// wide enough for the asterism's 26 degrees with margin, and an exposure open
/// enough that the seven dominate a field of fainter stars rather than sitting
/// among them.
fn dipper_camera(stars: &[Star]) -> Camera {
    Camera::new(400, 400)
        .looking_from([0.0; 3], dipper_aim(stars))
        .with_fov_degrees(36.0)
        .with_exposure(6.0)
        .with_seeing(1.8)
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name)
}

/// Decode a PNG into `(width, height, rgb8)`.
fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("a readable PNG");
    let mut buffer = vec![0; reader.output_buffer_size().expect("a bounded PNG")];
    let info = reader.next_frame(&mut buffer).expect("one frame");
    buffer.truncate(info.buffer_size());
    (info.width, info.height, buffer)
}

/// **The Big Dipper, drawn the same way twice.**
#[test]
fn the_dipper_matches_its_golden() {
    let stars = stars();
    let image = dipper_camera(&stars).render(&stars);
    let path = golden_path("dipper.png");

    if std::env::var_os("GALOS_SKY_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("a golden dir");
        image.write_png(&path).expect("writing the golden");
        eprintln!("blessed {}", path.display());
        return;
    }

    let expected = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("cannot read {}: {e}. Run with GALOS_SKY_BLESS=1 to create it.", path.display())
    });
    let (width, height, expected) = decode(&expected);
    assert_eq!((width, height), (image.width(), image.height()));

    let actual = image.to_srgb8();
    assert_eq!(actual.len(), expected.len());

    // A tolerance of one, because the golden is guarding this crate's physics
    // and not the last bit of a transcendental on some other architecture.
    let mut worst = 0i32;
    let mut differing = 0usize;
    for (a, b) in actual.iter().zip(&expected) {
        let d = (*a as i32 - *b as i32).abs();
        worst = worst.max(d);
        if d > 1 {
            differing += 1;
        }
    }
    if differing > 0 {
        let actual_path = std::env::temp_dir().join("dipper-actual.png");
        let _ = image.write_png(&actual_path);
        panic!(
            "{differing} of {} channels differ by more than one (worst {worst}). \
             Wrote what was drawn to {}. If the change is intended, \
             GALOS_SKY_BLESS=1 and look at the new picture.",
            actual.len(),
            actual_path.display(),
        );
    }
}

/// The seven land where the Dipper's shape says they should: a bowl of four
/// and a handle of three trailing off it.
///
/// The golden above would catch a projection that moved them, but it could not
/// say what was wrong. This says it, and it is what makes a failure of the
/// golden diagnosable rather than merely visible.
#[test]
fn the_dipper_has_the_shape_of_a_dipper() {
    let stars = stars();
    let camera = dipper_camera(&stars);
    let at = |name: &str| {
        let star = named(&stars, name);
        let (x, y, _) = camera.project(star.position).expect("in frame");
        (x, y)
    };

    // Every one of them lands inside the picture.
    for name in DIPPER {
        let (x, y) = at(name);
        assert!(
            (0.0..400.0).contains(&x) && (0.0..400.0).contains(&y),
            "{name} fell outside the frame at {x:.0},{y:.0}"
        );
    }

    // The handle runs monotonically away from the bowl: Megrez, Alioth, Mizar,
    // Alkaid, each further left than the last.
    let handle = ["Megrez", "Alioth", "Mizar", "Alkaid"];
    for pair in handle.windows(2) {
        assert!(
            at(pair[0]).0 > at(pair[1]).0,
            "{} should sit right of {}",
            pair[0],
            pair[1]
        );
    }

    // And the bowl is a quadrilateral, not a line: the four are not collinear.
    let (dx, dy) = at("Dubhe");
    let (mx, my) = at("Merak");
    let (px, py) = at("Phecda");
    let cross = (mx - dx) * (py - dy) - (my - dy) * (px - dx);
    assert!(cross.abs() > 1000.0, "the bowl came out flat: {cross}");
}
