mod app;
mod editor;
mod geometry;
mod groups;
mod history;
mod icons;
mod lens;
mod pin;
mod platform;
mod preferences;
mod raster;
mod selection;

fn main() {
    match app::parse_startup_action(std::env::args_os().skip(1)) {
        Ok(action) => {
            if let Err(error) = app::run(action) {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
