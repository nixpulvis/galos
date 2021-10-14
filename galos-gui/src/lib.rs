#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]  // Forbid warnings in release builds
#![warn(clippy::all, rust_2018_idioms)]

mod app;
pub use app::Galos;


#[cfg(target_arch = "wasm32")]
use eframe::wasm_bindgen::{self, prelude::*};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn start(canvas_id: &str) -> Result<(), eframe::wasm_bindgen::JsValue> {
    let app = Galos::default();
    eframe::start_web(canvas_id, Box::new(app))
}
