//! One sky, from one place, drawn on the CPU.
//!
//! Put a [`Camera`] somewhere, point it at something, hand it stars, get an
//! [`Image`]. That is the whole surface, and the smallness is the point: this
//! is the renderer you can reason about, so that the one built on a GPU can be
//! checked against something.
//!
//! # Why a second renderer
//!
//! `galos_photometry` has no way to be wrong on its own. Its tests check the
//! crate against itself — flux and magnitude undo each other, the main
//! sequence runs hot before cool — and none of them can catch a claim that is
//! self-consistent and false. Two things change that, and this is one of them:
//! a picture a person can look at and say *that is not Orion*. The other is a
//! real catalog, which is [`galos_catalog`], and the two are meant to be used
//! together — the fixture this crate's tests draw is eighty measured stars.
//!
//! The second reason is duller and matters more. The map will need a law
//! taking a magnitude to a drawn intensity, and developing one inside a
//! renderer with a GPU, a window and a swapchain in the loop is slow. Here it
//! is a pure function over a deterministic buffer.
//!
//! None of that law lives in this crate. [`galos_photometry::relative_exposure`]
//! turns a magnitude into energy and [`galos_photometry::psf`] decides where on
//! the detector it lands, so when the map adopts them the two renderers are
//! running one instrument rather than two, and a comparison between their
//! pictures measures the pictures. What is here is only the depositing: a loop
//! over the pixels a disc reaches, and a tone curve to compress the result. Those
//! are the parts a shader may legitimately do differently, which is why the
//! comparison in [`Image::total_energy`] is made on linear energy, before either
//! renderer's curve has touched it.
//!
//! # What it does not do
//!
//! No spatial index. A hundred thousand stars is a loop, and every star is
//! considered for every frame. That is not a shortcut, it is the requirement:
//! a renderer that pruned with the same octree the map prunes with could not
//! be used to check that octree, because both would be wrong together. If this
//! is ever pointed at 129 million systems it will read the index as a flat
//! store to get them, and still decide visibility itself.
//!
//! # Example
//!
//! ```no_run
//! use galos_sky::{Camera, Image};
//! use galos_catalog::hyg;
//! use std::fs::File;
//!
//! let (stars, _) = hyg::read(File::open("hygdata.csv")?)?;
//! let sirius = stars.iter().find(|s| s.name.as_deref() == Some("Sirius"));
//!
//! // Stand on Sol, look at Sirius, take the picture.
//! let camera = Camera::new(1600, 900)
//!     .looking_from([0.0; 3], sirius.unwrap().position)
//!     .with_fov_degrees(60.0)
//!     .with_exposure(1.0);
//! camera.render(&stars).write_png("sirius.png")?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod camera;
pub mod image;

pub use camera::Camera;
pub use image::Image;
