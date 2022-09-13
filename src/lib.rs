#![warn(clippy::all, rust_2018_idioms)]

pub use app::App;

mod app;
mod diff;
mod editops;
mod jobs;
mod obj;
mod views;
