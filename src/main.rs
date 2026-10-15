#![allow(clippy::needless_return)]

mod app;
mod capture;
mod editor;
mod geometry;
mod lens;
mod pin;
mod platform;
mod raster;
mod ui;

use clap::Parser;

fn main() {
    let args = std::env::args_os().collect::<Vec<_>>();
    let cli = app::cli::Cli::parse_from(&args);
    let result = platform::get_app_data_dir().and_then(|data_dir| {
        let listener = app::cli::connect_instance(&data_dir, args.get(1..).unwrap_or_default())?;
        return Ok(listener.map(|listener| (data_dir, listener)));
    });
    match result {
        Ok(Some((data_dir, listener))) => app::run(cli.command, &data_dir, listener),
        Ok(None) => {}
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    }
}
