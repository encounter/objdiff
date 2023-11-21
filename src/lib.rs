#![warn(clippy::all, rust_2018_idioms)]

pub use app::App;

mod app;
mod app_config;
mod config;
mod diff;
mod jobs;
mod obj;
mod update;
mod views;
