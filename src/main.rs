mod audio;
mod ui;

use std::process::ExitCode;

#[derive(Clone, Debug)]
pub struct Options {
    pub demo: bool,
    pub position: String,
    pub margin: i32,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wireplumber-widget: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut options = Options {
        demo: false,
        position: "top-right".into(),
        margin: 12,
    };
    let mut status = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "status" => status = true,
            "popup" => {}
            "--demo" => options.demo = true,
            "--position" => {
                options.position = args.next().ok_or("--position requires a corner")?;
                if !["top-right", "top-left", "bottom-right", "bottom-left"]
                    .contains(&options.position.as_str())
                {
                    return Err(
                        "position must be top-right, top-left, bottom-right, or bottom-left".into(),
                    );
                }
            }
            "--margin" => {
                options.margin = args
                    .next()
                    .ok_or("--margin requires pixels")?
                    .parse()
                    .map_err(|_| "margin must be an integer between 0 and 1000")?;
                if !(0..=1000).contains(&options.margin) {
                    return Err("margin must be between 0 and 1000".into());
                }
            }
            "--help" | "-h" => {
                println!(
                    "wireplumber-widget [popup|status] [OPTIONS]\n\n\
                    popup      Toggle the audio device menu (default)\n\
                    status     Print one Waybar JSON status line\n\n\
                    --demo     Use sample devices without changing system audio\n\
                    --position CORNER  top-right (default), top-left, bottom-right, bottom-left\n\
                    --margin PIXELS    Distance from the bar and screen edge (default: 12)\n\
                    -h, --help         Show this help"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg:?}; use --help")),
        }
    }
    if status {
        let result = if options.demo {
            Ok(audio::demo_snapshot())
        } else {
            audio::snapshot()
        };
        let payload = match result {
            Ok(snapshot) => audio::waybar(&snapshot),
            Err(error) => serde_json::json!({
                "text": "♪ Unavailable",
                "tooltip": gtk::glib::markup_escape_text(&error).as_str(),
                "class": "error",
                "alt": "unavailable"
            }),
        };
        // A valid status line keeps the module visible when PipeWire is down.
        println!("{payload}");
        Ok(())
    } else {
        ui::run(options)
    }
}
