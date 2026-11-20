mod audio;
mod ui;

use audio::{Kind, VolumeChange};
use std::process::ExitCode;

#[derive(Clone, Debug)]
pub struct Options {
    pub demo: bool,
    pub position: String,
    pub margin: i32,
    pub close_on_outside_click: bool,
    pub close_on_output_switch: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wpwidget: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut options = Options {
        demo: false,
        position: "top-right".into(),
        margin: 12,
        close_on_outside_click: true,
        close_on_output_switch: true,
    };
    let mut status = false;
    let mut mute = false;
    let mut volume = None;
    let mut kind = Kind::Output;
    let mut step = 5_u32;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "status" => status = true,
            "popup" => {}
            "mute" => mute = true,
            "volume" => {
                volume = Some(
                    args.next()
                        .ok_or("volume requires up, down, or a percentage")?,
                )
            }
            "--kind" => {
                kind = match args.next().as_deref() {
                    Some("output") => Kind::Output,
                    Some("input") => Kind::Input,
                    _ => return Err("--kind requires output or input".into()),
                };
            }
            "--step" => {
                step = args
                    .next()
                    .ok_or("--step requires a percentage")?
                    .parse()
                    .map_err(|_| "step must be an integer from 1 to 100")?;
                if !(1..=100).contains(&step) {
                    return Err("step must be between 1 and 100".into());
                }
            }
            "--demo" => options.demo = true,
            "--no-close-on-outside-click" => options.close_on_outside_click = false,
            "--no-close-on-output-switch" => options.close_on_output_switch = false,
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
                    "wpwidget [popup|status|mute|volume up|down|PERCENT] [OPTIONS]\n\n\
                    popup      Toggle the audio device menu (default)\n\
                    status     Print one Waybar JSON status line\n\n\
                    mute       Toggle mute on the default output (or --kind input)\n\
                    volume     Adjust the default volume (up/down steps default to 5%)\n\n\
                    --kind output|input   Target for status, mute, and volume\n\
                    --step PERCENT        Scroll/volume step (default: 5)\n\
                    --demo     Use sample devices without changing system audio\n\
                    --position CORNER  top-right (default), top-left, bottom-right, bottom-left\n\
                    --margin PIXELS    Distance from the bar and screen edge (default: 12)\n\
                    --no-close-on-outside-click  Keep the menu open when clicking outside it\n\
                    --no-close-on-output-switch  Keep the menu open after switching outputs\n\
                    -h, --help         Show this help"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg:?}; use --help")),
        }
    }
    if usize::from(status) + usize::from(mute) + usize::from(volume.is_some()) > 1 {
        return Err("choose one command: popup, status, mute, or volume".into());
    }
    if mute || volume.is_some() {
        if options.demo {
            return Err(
                "mute and volume commands are unavailable in demo mode; use popup --demo".into(),
            );
        }
        return if mute {
            audio::toggle_default_mute(kind)
        } else {
            let change = match volume.as_deref().unwrap() {
                "up" => VolumeChange::Up(step),
                "down" => VolumeChange::Down(step),
                value => VolumeChange::Set(
                    value
                        .parse()
                        .map_err(|_| "volume requires up, down, or an integer from 0 to 100")?,
                ),
            };
            audio::change_default_volume(kind, change)
        };
    }
    if status {
        let result = if options.demo {
            Ok(audio::demo_snapshot())
        } else {
            audio::snapshot()
        };
        let payload = match result {
            Ok(snapshot) => audio::waybar(&snapshot, kind),
            Err(error) => serde_json::json!({
                "text": "Unavailable",
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
