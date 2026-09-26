//! A 3D Galaxy Map for `galos`
//!
//! ![](https://github.com/nixpulvis/galos/blob/master/galos_map/demo.gif?raw=true)
//!
//! Requires a built `galos_index` directory: the cell tree and the metadata
//! sidecars beside it, read through one [`galos_index::Source`].
//!
//! Two plugins and a third over both. [`map::plugin`] draws the galaxy and
//! the camera that flies it, and knows nothing of the chrome over it.
//! [`ui::plugin`] is that chrome — the settings pane, the bar, the panels,
//! the clock — and works the map through the resources and messages the
//! map owns. [`dev::plugin`] is a diagnostics window over both.

pub mod dev;
pub mod map;
#[cfg(test)]
pub(crate) mod testing;
pub mod ui;
