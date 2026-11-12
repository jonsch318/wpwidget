# WirePlumber Widget

A small Rust + GTK4 audio switcher for Waybar, styled after GNOME's quick settings.
Click the bar module to open a rounded dark panel with separate **Sound Output**
and **Sound Input** sections. Click a device to make it the default; a blue row
and checkmark show the active selection. The menu stays open so you can change
both sections. Escape, the close button, or another click on the module closes it.
While switching, only the clicked row shows a spinner; it fades to a checkmark
when WirePlumber confirms the change. Other device icons remain unchanged.

The device list refreshes every two seconds, including USB/Bluetooth hotplug and
defaults changed by other applications. Audio queries run off the GTK thread.
The bar displays the output name, with both defaults in its tooltip.

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

## Waybar setup

Merge [examples/waybar.jsonc](examples/waybar.jsonc) into your Waybar config
(usually `~/.config/waybar/config.jsonc`). Add `custom/audio-devices` to your
existing `modules-right`, `modules-left`, or `modules-center` array. Append
[examples/waybar.css](examples/waybar.css) to your Waybar stylesheet, then restart
Waybar using your usual session/compositor launcher.

The example uses `~/.local/bin/wireplumber-widget`; adjust both commands if you
install elsewhere. There is no font-icon dependency for the bar's `♪` label.

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
