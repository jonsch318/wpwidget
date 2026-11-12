# WirePlumber Widget

A small Rust + GTK4 audio switcher for Waybar, styled after GNOME's quick settings.
Click the bar module to open a rounded dark panel with separate **Sound Output**
and **Sound Input** sections. Click a device to make it the default; a blue row
and checkmark show the active selection. The menu stays open so you can change
both sections. Escape, the close button, or another click on the module closes it.
Each section has a compact volume slider and an icon button to mute/unmute its
current default device. Output and microphone levels are independent. Sliders
adjust from 0–100% and preserve the mute state; click the icon to unmute.
While switching, only the clicked row shows a spinner; it fades to a checkmark
when WirePlumber confirms the change. Other device icons remain unchanged.

The device list refreshes every two seconds, including USB/Bluetooth hotplug and
defaults changed by other applications. Audio queries run off the GTK thread.
The bar displays the volume with device/volume icons, Bluetooth and mute states,
with both defaults and levels in its tooltip. It replaces the built-in Waybar
WirePlumber module while adding the device picker and microphone controls.

## Requirements

- Rust/Cargo (edition 2024 compatible), a C compiler, and `pkg-config`.
- GTK4 development libraries and gtk4-layer-shell development libraries.
- PipeWire (`pw-dump`) and WirePlumber (`wpctl`), running in your user session.
- Waybar and a Wayland compositor supporting layer-shell, such as Sway, Hyprland,
  or Niri. A session D-Bus is needed for the single-instance menu toggle.
- A symbolic icon theme such as Adwaita.

On Arch Linux the native packages are `gtk4 gtk4-layer-shell pipewire wireplumber
waybar adwaita-icon-theme base-devel rust`. On Debian/Ubuntu with GTK4 layer-shell
available, the development packages are `libgtk-4-dev libgtk4-layer-shell-dev`,
alongside `build-essential pkg-config cargo pipewire-bin wireplumber waybar
adwaita-icon-theme`.

## Build and install

```sh
cargo build --release
install -Dm755 target/release/wireplumber-widget ~/.local/bin/wireplumber-widget
```

Installation is manual; building does not modify your Waybar configuration.

## GitHub Actions builds

[`.github/workflows/build.yml`](.github/workflows/build.yml) runs on pushes, pull
requests, and manual dispatches. It checks formatting and Clippy, runs all tests
(including GTK controls under Xvfb), and builds an optimized Linux x86_64 binary.
The runner builds the native GTK4 layer-shell library at version 1.3.0.

Download `wireplumber-widget-linux-x86_64` from a successful run's **Artifacts**.
Extract the included `.tar.gz` to preserve executable permissions; it contains the
binary, this README, and Waybar examples. A SHA-256 checksum is included.
The binary needs GTK4 and GTK4 layer-shell runtime libraries on the target system,
plus PipeWire and WirePlumber; it is not statically linked. Builds use Ubuntu
24.04, so the target needs a compatible glibc (2.39 or newer).

## Waybar setup

Merge [examples/waybar.jsonc](examples/waybar.jsonc) into your Waybar config
(usually `~/.config/waybar/config.jsonc`). Add `custom/audio-devices` to your
existing `modules-right`, `modules-left`, or `modules-center` array. Append
[examples/waybar.css](examples/waybar.css) to your Waybar stylesheet, then restart
Waybar using your usual session/compositor launcher.
Remove `wireplumber` from the module list to avoid duplicate controls.

The example uses `~/.local/bin/wireplumber-widget`; adjust both commands if you
install elsewhere. The example uses the same Nerd Font icons as your existing
WirePlumber configuration; install a Nerd Font or change `format-icons`.

| Bar action | Control |
| --- | --- |
| Left click | Open/close the menu |
| Right click | Toggle output mute |
| Middle click | Toggle microphone mute |
| Scroll up/down | Output volume ±5% |

`format-icons` supports the existing `headphone`, `hands-free`, `headset`, `phone`,
`portable`, `car`, and `default` definitions, plus `muted` and `unavailable`.
The default array chooses low/medium/high icons by volume. Device form factors
come from PipeWire device properties. Bluetooth adds the existing `` suffix.
CSS classes include `muted`, `bluetooth`, `source-muted` (microphone muted),
`connected`, `disconnected`, and the device form factor. Reuse existing Waybar
styles by changing their selector from `#wireplumber` to `#custom-audio-devices`.

This is a custom module, so `format` uses `{icon}` and `{text}` (the volume),
instead of the built-in module's `{volume}`. Use `--step 5` in scroll commands
instead of `scroll-step`. Muted/Bluetooth formatting is provided by status JSON;
the built-in `format-muted` and `format-bluetooth` keys do not apply to custom
modules. [examples/waybar.jsonc](examples/waybar.jsonc) is a complete translation.

For a separate microphone display with its own mute and scroll controls, merge
[examples/waybar-input.jsonc](examples/waybar-input.jsonc) and add
`custom/audio-input` to your module list. Both modules open the same menu.

The menu defaults to the top-right corner, inset 12 pixels beyond the compositor's
reserved bar area. For a bottom bar, change `on-click` to:

```sh
$HOME/.local/bin/wireplumber-widget popup --position bottom-right --margin 12
```

Positions: `top-right`, `top-left`, `bottom-right`, `bottom-left`. On compositors
without layer-shell (including GNOME Shell), the menu opens as a normal window;
corner positioning is only available with layer-shell. This is a GNOME-style
widget, not a GNOME Shell extension.

## Preview and development

```sh
cargo run -- popup --demo     # Interactive sample devices; leaves audio unchanged
cargo run -- status --demo    # Example Waybar JSON without an audio server
cargo run -- popup            # Real devices; selecting one changes its default
cargo run -- status           # One JSON line for Waybar
cargo run -- status --kind input # Microphone status
cargo run -- volume up --step 5  # Raise output volume, capped at 100%
cargo run -- volume 40 --kind input # Set microphone volume to 40%
cargo run -- mute --kind input   # Toggle microphone mute
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test -- --ignored --test-threads=1  # GTK regression test; needs a display
```

The demo menu has its own application ID, so it can run alongside the real menu.
GTK styling is embedded from `src/style.css`; rebuild after changing it.

## How switching works

`pw-dump` supplies structured device data and effective default metadata.
`wpctl set-default ID` selects the output or input independently. Stable node
names are resolved immediately before switching to handle device reconnections.
`wpctl get-volume`, `set-volume`, and `set-mute` read and control levels. Slider
updates are coalesced while the worker is busy so dragging does not build a queue
of stale volume changes. External changes appear on the next refresh.
WirePlumber controls persistence and routing policy. New streams use the chosen
default; existing applications may retain an explicitly selected device or need
playback restarted. This tool does not force-move application streams, change
card profiles, pair Bluetooth devices, or expose disconnected hardware.

If the module shows **Unavailable**, click it for the error or run `pw-dump` and
`wpctl status` from your desktop terminal. Run the widget as your desktop user,
with the same session environment as Waybar. No `sudo` is needed.

References: [Waybar custom modules](https://github.com/Alexays/Waybar/wiki/Module:-Custom),
[wpctl](https://pipewire.pages.freedesktop.org/wireplumber/man/wpctl.html),
and [GTK4 layer-shell](https://github.com/wmww/gtk4-layer-shell).
