// A window program: no console. (The release build has no console window at all.)
#![cfg_attr(windows, windows_subsystem = "windows")]

mod applog;
mod backup;
mod culture;
mod date;
mod export;
mod format;
mod fsops;
mod model;
mod netversion;
mod pnputil;
mod proc;
mod settings;

#[cfg_attr(not(windows), allow(dead_code))]
mod native;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
mod win;

#[cfg(windows)]
mod startup;

#[cfg(windows)]
fn main() {
    startup::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("DriverStore Manager only runs on Windows.");
    std::process::exit(1);
}
