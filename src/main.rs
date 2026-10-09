mod app;
mod editor;
mod geometry;
mod history;
mod icons;
mod lens;
mod pin;
mod platform;
mod raster;
mod selection;

fn main() {
    match app::parse_startup_action(std::env::args_os().skip(1)) {
        Ok(action) => app::run(action),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
