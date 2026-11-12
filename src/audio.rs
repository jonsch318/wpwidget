use serde_json::{Value, json};
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: u64,
    pub name: String,
    pub label: String,
    pub kind: Kind,
    pub form_factor: String,
    pub bluetooth: bool,
}

impl Device {
    pub fn icon(&self) -> &'static str {
        match (self.kind, self.form_factor.as_str()) {
            (Kind::Input, _) => "audio-input-microphone-symbolic",
            (_, "headphone") => "audio-headphones-symbolic",
            (_, "headset" | "hands-free") => "audio-headset-symbolic",
            _ => "audio-speakers-symbolic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Output,
    Input,
}

impl Kind {
    pub fn default_id(self) -> &'static str {
        match self {
            Self::Output => "@DEFAULT_AUDIO_SINK@",
            Self::Input => "@DEFAULT_AUDIO_SOURCE@",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Output => 0,
            Self::Input => 1,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Output => "default.audio.sink",
            Self::Input => "default.audio.source",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub percent: u32,
    pub muted: bool,
}

pub fn parse_volume(text: &str) -> Result<Volume, String> {
    let mut parts = text.split_whitespace();
    if parts.next() != Some("Volume:") {
        return Err(format!("Unexpected wpctl volume response: {text}"));
    }
    let level: f64 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .filter(|level: &f64| level.is_finite() && *level >= 0.0)
        .ok_or_else(|| format!("Invalid volume response: {text}"))?;
    Ok(Volume {
        percent: (level * 100.0).round() as u32,
        muted: parts.any(|part| part == "[MUTED]"),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeChange {
    Set(u32),
    Up(u32),
    Down(u32),
}

impl VolumeChange {
    pub fn argument(self) -> Result<String, String> {
        let (value, suffix) = match self {
            Self::Set(value) => (value, ""),
            Self::Up(value) => (value, "+"),
            Self::Down(value) => (value, "-"),
        };
        if value > 100 {
            return Err("Volume must be between 0 and 100 percent".into());
        }
        Ok(format!("{value}%{suffix}"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub output: Option<String>,
    pub input: Option<String>,
    pub volumes: [Option<Volume>; 2],
}

impl Snapshot {
    pub fn volume(&self, kind: Kind) -> Option<Volume> {
        self.volumes[kind.index()]
    }

    pub fn default_device(&self, kind: Kind) -> Option<&Device> {
        let name = match kind {
            Kind::Output => self.output.as_deref(),
            Kind::Input => self.input.as_deref(),
        };
        self.devices
            .iter()
            .find(|device| device.kind == kind && Some(device.name.as_str()) == name)
    }

    pub fn is_default(&self, device: &Device) -> bool {
        self.default_device(device.kind) == Some(device)
    }
}

/// Read structured PipeWire data instead of parsing localized wpctl status text.
pub fn parse_snapshot(data: &str) -> Result<Snapshot, String> {
    let objects: Vec<Value> = serde_json::from_str(data)
        .map_err(|err| format!("Could not read PipeWire device data: {err}"))?;
    let mut snapshot = Snapshot::default();
    for object in &objects {
        let props = &object["info"]["props"];
        if object["type"] == "PipeWire:Interface:Node" {
            let kind = match props["media.class"].as_str() {
                Some("Audio/Sink") => Kind::Output,
                Some("Audio/Source") | Some("Audio/Source/Virtual") => Kind::Input,
                _ => continue,
            };
            let (Some(id), Some(name)) = (object["id"].as_u64(), props["node.name"].as_str())
            else {
                continue;
            };
            let label = ["node.description", "node.nick", "node.name"]
                .iter()
                .filter_map(|key| props[key].as_str())
                .find(|label| !label.is_empty())
                .unwrap_or(name);
            let parent_id = props["device.id"]
                .as_u64()
                .or_else(|| props["device.id"].as_str().and_then(|id| id.parse().ok()));
            let parent = objects.iter().find(|candidate| {
                candidate["type"] == "PipeWire:Interface:Device"
                    && candidate["id"].as_u64() == parent_id
            });
            let parent_props = parent.map(|parent| &parent["info"]["props"]);
            let property = |key: &str| {
                props[key]
                    .as_str()
                    .or_else(|| parent_props.and_then(|props| props[key].as_str()))
            };
            snapshot.devices.push(Device {
                id,
                name: name.to_owned(),
                label: label.to_owned(),
                kind,
                form_factor: property("device.form-factor")
                    .unwrap_or("default")
                    .to_owned(),
                bluetooth: property("device.api").is_some_and(|api| api.starts_with("bluez"))
                    || name.starts_with("bluez_"),
            });
        }
        if object["type"] == "PipeWire:Interface:Metadata"
            && object["props"]["metadata.name"] == "default"
            && let Some(entries) = object["metadata"].as_array()
        {
            for entry in entries {
                if entry["subject"].as_u64() != Some(0) {
                    continue;
                }
                let value = match entry["value"].as_str() {
                    Some(raw) => serde_json::from_str(raw).unwrap_or(Value::Null),
                    None => entry["value"].clone(),
                };
                let name = value["name"].as_str().map(str::to_owned);
                match entry["key"].as_str() {
                    Some("default.audio.sink") => snapshot.output = name,
                    Some("default.audio.source") => snapshot.input = name,
                    _ => {}
                }
            }
        }
    }
    snapshot.devices.sort_by(|a, b| {
        a.label
            .to_lowercase()
            .cmp(&b.label.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    Ok(snapshot)
}

fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Cannot start {program}: {err}. Check that it is installed."))?;
    // Drain both pipes concurrently so a large device graph cannot fill a pipe.
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match result {
                    Err(error) => format!("Could not wait for {program}: {error}"),
                    _ => format!("{program} timed out. Is PipeWire running?"),
                });
            }
        }
    };
    let stdout = out.join().unwrap().map_err(|err| err.to_string())?;
    let stderr = err.join().unwrap().map_err(|err| err.to_string())?;
    let status = status?;
    if !status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    String::from_utf8(stdout).map_err(|err| err.to_string())
}

pub fn snapshot() -> Result<Snapshot, String> {
    let mut result = graph_snapshot()?;
    for kind in [Kind::Output, Kind::Input] {
        if let Some(device) = result.default_device(kind) {
            result.volumes[kind.index()] = Some(parse_volume(&command(
                "wpctl",
                &["get-volume", &device.id.to_string()],
            )?)?);
        }
    }
    Ok(result)
}

fn graph_snapshot() -> Result<Snapshot, String> {
    parse_snapshot(&command("pw-dump", &["--no-colors"])?)
}

pub fn set_default(device: &Device) -> Result<Snapshot, String> {
    set_default_with(device, graph_snapshot, |id| {
        command("wpctl", &["set-default", &id.to_string()]).map(|_| ())
    })?;
    snapshot()
}

fn resolve(device: &Device) -> Result<u64, String> {
    graph_snapshot()?
        .devices
        .iter()
        .find(|candidate| candidate.name == device.name && candidate.kind == device.kind)
        .map(|candidate| candidate.id)
        .ok_or_else(|| "This device was disconnected. Choose another device.".into())
}

pub fn set_volume(device: &Device, percent: u32) -> Result<Snapshot, String> {
    let argument = VolumeChange::Set(percent).argument()?;
    command(
        "wpctl",
        &[
            "set-volume",
            &resolve(device)?.to_string(),
            &argument,
            "--limit",
            "1.0",
        ],
    )?;
    snapshot()
}

pub fn toggle_mute(device: &Device) -> Result<Snapshot, String> {
    command(
        "wpctl",
        &["set-mute", &resolve(device)?.to_string(), "toggle"],
    )?;
    snapshot()
}

pub fn change_default_volume(kind: Kind, change: VolumeChange) -> Result<(), String> {
    command(
        "wpctl",
        &[
            "set-volume",
            kind.default_id(),
            &change.argument()?,
            "--limit",
            "1.0",
        ],
    )
    .map(|_| ())
}

pub fn toggle_default_mute(kind: Kind) -> Result<(), String> {
    command("wpctl", &["set-mute", kind.default_id(), "toggle"]).map(|_| ())
}

fn set_default_with(
    device: &Device,
    mut read: impl FnMut() -> Result<Snapshot, String>,
    mut select: impl FnMut(u64) -> Result<(), String>,
) -> Result<Snapshot, String> {
    // IDs are ephemeral: resolve the stable node name again after a hotplug.
    let current = read()?;
    let target = current
        .devices
        .iter()
        .find(|candidate| candidate.name == device.name && candidate.kind == device.kind)
        .ok_or_else(|| "This device was disconnected. Choose another device.".to_owned())?;
    select(target.id)?;
    // Metadata propagation can lag behind wpctl returning.
    for _ in 0..10 {
        let current = read()?;
        if current
            .default_device(device.kind)
            .is_some_and(|d| d.name == device.name)
        {
            return Ok(current);
        }
        thread::sleep(Duration::from_millis(75));
    }
    Err(format!(
        "WirePlumber has not confirmed {}. Try again.",
        device.kind.key()
    ))
}

pub fn waybar(snapshot: &Snapshot, kind: Kind) -> Value {
    let output = snapshot.default_device(Kind::Output);
    let input = snapshot.default_device(Kind::Input);
    let device = snapshot.default_device(kind);
    let volume = snapshot.volume(kind);
    let muted = volume.is_some_and(|volume| volume.muted);
    let bluetooth = device.is_some_and(|device| device.bluetooth);
    let form = device.map_or("default", |device| device.form_factor.as_str());
    let mut classes = vec![if device.is_some() {
        "connected"
    } else {
        "disconnected"
    }];
    if muted {
        classes.push("muted");
    }
    if bluetooth {
        classes.push("bluetooth");
    }
    classes.push(form);
    if snapshot
        .volume(Kind::Input)
        .is_some_and(|volume| volume.muted)
    {
        classes.push("source-muted");
    }
    let describe = |device: Option<&Device>, kind| {
        device.map_or("Not connected".into(), |device| {
            let volume = snapshot.volume(kind).map_or(String::new(), |v| {
                format!(" — {}%{}", v.percent, if v.muted { " (muted)" } else { "" })
            });
            format!("{}{volume}", device.label)
        })
    };
    let text = if device.is_none() {
        "Unavailable".into()
    } else if muted {
        String::new()
    } else {
        volume.map_or("—".into(), |v| format!("{}%", v.percent))
    };
    json!({
        "text": format!("{text}{}", if bluetooth { " " } else { "" }),
        "tooltip": gtk::glib::markup_escape_text(&format!(
            "Output: {}\nInput: {}\n\nClick for audio controls",
            describe(output, Kind::Output), describe(input, Kind::Input)
        )).as_str(),
        "class": classes,
        "alt": if device.is_none() { "unavailable" } else if muted { "muted" } else { form },
        "percentage": volume.map_or(0, |volume| volume.percent.min(100))
    })
}

pub fn demo_snapshot() -> Snapshot {
    Snapshot {
        devices: vec![
            Device {
                id: 1,
                name: "speakers".into(),
                label: "Built-in Speakers".into(),
                kind: Kind::Output,
                form_factor: "default".into(),
                bluetooth: false,
            },
            Device {
                id: 2,
                name: "headphones".into(),
                label: "Studio Headphones".into(),
                kind: Kind::Output,
                form_factor: "headphone".into(),
                bluetooth: false,
            },
            Device {
                id: 3,
                name: "display".into(),
                label: "HDMI / DisplayPort".into(),
                kind: Kind::Output,
                form_factor: "default".into(),
                bluetooth: false,
            },
            Device {
                id: 4,
                name: "microphone".into(),
                label: "Built-in Microphone".into(),
                kind: Kind::Input,
                form_factor: "default".into(),
                bluetooth: false,
            },
            Device {
                id: 5,
                name: "usb-mic".into(),
                label: "USB Microphone".into(),
                kind: Kind::Input,
                form_factor: "default".into(),
                bluetooth: false,
            },
        ],
        output: Some("headphones".into()),
        input: Some("microphone".into()),
        volumes: [
            Some(Volume {
                percent: 65,
                muted: false,
            }),
            Some(Volume {
                percent: 80,
                muted: false,
            }),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_input_resolves_new_id_and_keeps_output_default() {
        let before = demo_snapshot();
        let target = before.devices[4].clone();
        let mut reconnected = before.clone();
        reconnected.devices[4].id = 123;
        let mut after = reconnected.clone();
        after.input = Some(target.name.clone());
        let mut snapshots = vec![reconnected, after.clone()].into_iter();
        let mut selected_id = None;
        let result = set_default_with(
            &target,
            || Ok(snapshots.next().unwrap()),
            |id| {
                selected_id = Some(id);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(selected_id, Some(123));
        assert_eq!(result.input, Some(target.name));
        assert_eq!(result.output, before.output);
    }

    #[test]
    fn disconnected_device_does_not_select_a_reused_id() {
        let before = demo_snapshot();
        let target = before.devices[0].clone();
        let mut after = before;
        after.devices[0].name = "replacement".into();
        let mut wrote = false;
        let result = set_default_with(
            &target,
            || Ok(after.clone()),
            |_| {
                wrote = true;
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("disconnected"));
        assert!(!wrote);
    }

    #[test]
    fn switching_failure_is_reported() {
        let before = demo_snapshot();
        let target = before.devices[0].clone();
        let result = set_default_with(
            &target,
            || Ok(before.clone()),
            |_| Err("Device unavailable".into()),
        );
        assert_eq!(result.unwrap_err(), "Device unavailable");
    }

    #[test]
    fn reads_realistic_graph_and_effective_defaults() {
        let snapshot = parse_snapshot(include_str!("../tests/graph.json")).unwrap();
        assert_eq!(snapshot.devices.len(), 3);
        assert_eq!(snapshot.default_device(Kind::Output).unwrap().id, 42);
        assert_eq!(snapshot.default_device(Kind::Input).unwrap().id, 51);
        assert_eq!(snapshot.devices[0].label, "Headphones & DAC <USB>");
        assert!(snapshot.is_default(snapshot.default_device(Kind::Input).unwrap()));
    }

    #[test]
    fn excludes_streams_and_handles_removed_defaults() {
        let snapshot = parse_snapshot(r#"[
            {"id":3,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Stream/Output/Audio","node.name":"music"}}},
            {"type":"PipeWire:Interface:Metadata","props":{"metadata.name":"default"},"metadata":[
                {"subject":0,"key":"default.audio.sink","value":"{\"name\":\"unplugged\"}"}
            ]}
        ]"#).unwrap();
        assert!(snapshot.devices.is_empty());
        assert!(snapshot.default_device(Kind::Output).is_none());
        assert!(
            waybar(&snapshot, Kind::Output)["class"]
                .as_array()
                .unwrap()
                .contains(&json!("disconnected"))
        );
    }

    #[test]
    fn invalid_data_is_reported_and_empty_graph_is_valid() {
        assert!(parse_snapshot("not json").is_err());
        assert!(parse_snapshot("{}").is_err());
        assert_eq!(parse_snapshot("[]").unwrap(), Snapshot::default());
    }

    #[test]
    fn waybar_escapes_markup_and_reports_volume_and_mute() {
        let mut snapshot = parse_snapshot(include_str!("../tests/graph.json")).unwrap();
        snapshot.volumes = [
            Some(Volume {
                percent: 65,
                muted: false,
            }),
            Some(Volume {
                percent: 80,
                muted: true,
            }),
        ];
        let payload = waybar(&snapshot, Kind::Output);
        assert!(
            payload["tooltip"]
                .as_str()
                .unwrap()
                .contains("&amp; DAC &lt;USB&gt;")
        );
        assert_eq!(payload["text"], "65%");
        assert_eq!(payload["percentage"], 65);
        assert_eq!(payload["alt"], "headphone");
        assert!(
            payload["class"]
                .as_array()
                .unwrap()
                .contains(&json!("source-muted"))
        );
        let input = waybar(&snapshot, Kind::Input);
        assert_eq!(input["alt"], "muted");
        assert_eq!(input["text"], "");
        snapshot.volumes[0] = Some(Volume {
            percent: 125,
            muted: false,
        });
        assert_eq!(waybar(&snapshot, Kind::Output)["percentage"], 100);
        assert_eq!(waybar(&snapshot, Kind::Output)["text"], "125%");
    }

    #[test]
    fn parses_volume_without_losing_mute_or_amplified_levels() {
        assert_eq!(
            parse_volume("Volume: 0.65\n").unwrap(),
            Volume {
                percent: 65,
                muted: false
            }
        );
        assert_eq!(
            parse_volume("Volume: 1.25 [MUTED]\n").unwrap(),
            Volume {
                percent: 125,
                muted: true
            }
        );
        for invalid in ["", "Volume: NaN", "Volume: inf", "Volume: -1", "error"] {
            assert!(parse_volume(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn volume_arguments_support_both_defaults_and_bounded_steps() {
        assert_eq!(Kind::Output.default_id(), "@DEFAULT_AUDIO_SINK@");
        assert_eq!(Kind::Input.default_id(), "@DEFAULT_AUDIO_SOURCE@");
        assert_eq!(VolumeChange::Up(5).argument().unwrap(), "5%+");
        assert_eq!(VolumeChange::Down(5).argument().unwrap(), "5%-");
        assert_eq!(VolumeChange::Set(0).argument().unwrap(), "0%");
        assert!(VolumeChange::Set(101).argument().is_err());
    }

    #[test]
    fn bluetooth_and_form_factor_are_inherited_from_parent_device() {
        let mut objects: Value = serde_json::from_str(include_str!("../tests/graph.json")).unwrap();
        objects[0]["info"]["props"]["device.api"] = json!("bluez5");
        let mut snapshot = parse_snapshot(&objects.to_string()).unwrap();
        snapshot.volumes[0] = Some(Volume {
            percent: 50,
            muted: true,
        });
        let device = snapshot.default_device(Kind::Output).unwrap();
        assert!(device.bluetooth);
        assert_eq!(device.form_factor, "headphone");
        assert_eq!(device.icon(), "audio-headphones-symbolic");
        let payload = waybar(&snapshot, Kind::Output);
        assert!(
            payload["class"]
                .as_array()
                .unwrap()
                .contains(&json!("bluetooth"))
        );
        assert!(
            payload["class"]
                .as_array()
                .unwrap()
                .contains(&json!("muted"))
        );
        assert_eq!(payload["alt"], "muted");
    }
}
