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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Output,
    Input,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Self::Output => "default.audio.sink",
            Self::Input => "default.audio.source",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub output: Option<String>,
    pub input: Option<String>,
}

impl Snapshot {
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
    for object in objects {
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
            snapshot.devices.push(Device {
                id,
                name: name.to_owned(),
                label: label.to_owned(),
                kind,
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
    parse_snapshot(&command("pw-dump", &["--no-colors"])?)
}

pub fn set_default(device: &Device) -> Result<Snapshot, String> {
    set_default_with(device, snapshot, |id| {
        command("wpctl", &["set-default", &id.to_string()]).map(|_| ())
    })
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

pub fn waybar(snapshot: &Snapshot) -> Value {
    let output = snapshot.default_device(Kind::Output);
    let input = snapshot.default_device(Kind::Input);
    let label = output.map_or("No output", |device| device.label.as_str());
    let short: String = label.chars().take(32).collect();
    let short = if label.chars().count() > 32 {
        format!("{short}…")
    } else {
        short
    };
    json!({
        "text": format!("♪ {short}"),
        "tooltip": gtk::glib::markup_escape_text(&format!(
            "Output: {label}\nInput: {}\n\nClick to switch audio devices",
            input.map_or("No input", |device| device.label.as_str())
        )).as_str(),
        "class": if output.is_some() { "connected" } else { "disconnected" },
        "alt": "audio"
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
            },
            Device {
                id: 2,
                name: "headphones".into(),
                label: "Studio Headphones".into(),
                kind: Kind::Output,
            },
            Device {
                id: 3,
                name: "display".into(),
                label: "HDMI / DisplayPort".into(),
                kind: Kind::Output,
            },
            Device {
                id: 4,
                name: "microphone".into(),
                label: "Built-in Microphone".into(),
                kind: Kind::Input,
            },
            Device {
                id: 5,
                name: "usb-mic".into(),
                label: "USB Microphone".into(),
                kind: Kind::Input,
            },
        ],
        output: Some("headphones".into()),
        input: Some("microphone".into()),
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
        assert_eq!(waybar(&snapshot)["class"], "disconnected");
    }

    #[test]
    fn invalid_data_is_reported_and_empty_graph_is_valid() {
        assert!(parse_snapshot("not json").is_err());
        assert!(parse_snapshot("{}").is_err());
        assert_eq!(parse_snapshot("[]").unwrap(), Snapshot::default());
    }

    #[test]
    fn waybar_escapes_markup_and_handles_unicode_names() {
        let mut snapshot = parse_snapshot(include_str!("../tests/graph.json")).unwrap();
        let payload = waybar(&snapshot);
        assert!(
            payload["tooltip"]
                .as_str()
                .unwrap()
                .contains("&amp; DAC &lt;USB&gt;")
        );
        assert!(
            payload["text"]
                .as_str()
                .unwrap()
                .contains("Headphones & DAC <USB>")
        );
        snapshot
            .devices
            .iter_mut()
            .for_each(|device| device.label = "🎧".repeat(50));
        assert!(waybar(&snapshot)["text"].as_str().unwrap().ends_with('…'));
    }
}
