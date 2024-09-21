#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]  // Forbid warnings in release builds
#![warn(clippy::all, rust_2018_idioms)]


#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let app = galos_gui::Galos::default();
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(Box::new(app), native_options);
}

#[cfg(target_arch = "wasm32")]
fn main() {
    unimplemented!()
}
