use crate::Options;
use crate::audio::{self, Device, Kind, Snapshot};
use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

enum Request {
    Refresh,
    Select(Device),
    Volume(Device, u32),
    Mute(Device),
}

struct View {
    output: gtk::Box,
    input: gtk::Box,
    message: gtk::Label,
    busy: Cell<bool>,
    snapshot: RefCell<Option<Snapshot>>,
    rows: RefCell<Vec<DeviceRow>>,
    switch_started: Cell<Option<Instant>>,
    controls: [VolumeControl; 2],
    queue: RefCell<VecDeque<Request>>,
    sender: Sender<Request>,
}

struct VolumeControl {
    container: gtk::Box,
    mute: gtk::Button,
    scale: gtk::Scale,
    label: gtk::Label,
    updating: Cell<bool>,
    device: RefCell<Option<Device>>,
}

impl VolumeControl {
    fn new(kind: Kind) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        container.add_css_class("volume-control");
        let mute = gtk::Button::from_icon_name(match kind {
            Kind::Output => "audio-volume-high-symbolic",
            Kind::Input => "audio-input-microphone-symbolic",
        });
        mute.add_css_class("mute-button");
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.set_tooltip_text(Some(match kind {
            Kind::Output => "Output volume",
            Kind::Input => "Microphone volume",
        }));
        let label = gtk::Label::new(Some("—"));
        label.set_width_chars(4);
        label.set_xalign(1.0);
        label.add_css_class("volume-value");
        container.append(&mute);
        container.append(&scale);
        container.append(&label);
        container.set_sensitive(false);
        Self {
            container,
            mute,
            scale,
            label,
            updating: Cell::new(false),
            device: RefCell::new(None),
        }
    }

    fn sync(&self, snapshot: &Snapshot, kind: Kind, pending: bool) {
        let device = snapshot.default_device(kind);
        let volume = snapshot.volume(kind);
        self.container
            .set_sensitive(device.is_some() && volume.is_some());
        *self.device.borrow_mut() = device.cloned();
        if let Some(volume) = volume {
            let icon = match (kind, volume.muted, volume.percent) {
                (Kind::Input, true, _) => "microphone-sensitivity-muted-symbolic",
                (Kind::Input, false, _) => "audio-input-microphone-symbolic",
                (Kind::Output, true, _) => "audio-volume-muted-symbolic",
                (Kind::Output, false, 0) => "audio-volume-muted-symbolic",
                (Kind::Output, false, 1..=33) => "audio-volume-low-symbolic",
                (Kind::Output, false, 34..=66) => "audio-volume-medium-symbolic",
                _ => "audio-volume-high-symbolic",
            };
            self.mute.set_icon_name(icon);
            self.mute.set_tooltip_text(Some(match (kind, volume.muted) {
                (Kind::Output, false) => "Mute output",
                (Kind::Output, true) => "Unmute output",
                (Kind::Input, false) => "Mute microphone",
                (Kind::Input, true) => "Unmute microphone",
            }));
            if volume.muted {
                self.mute.add_css_class("muted");
            } else {
                self.mute.remove_css_class("muted");
            }
            if !pending {
                self.updating.set(true);
                self.scale.set_value(volume.percent.min(100) as f64);
                self.label.set_text(&format!("{}%", volume.percent));
                self.updating.set(false);
            }
        } else {
            self.label.set_text("—");
        }
    }
}

struct DeviceRow {
    device: Device,
    button: gtk::Button,
    indicator: gtk::Stack,
    spinner: gtk::Spinner,
    check: gtk::Image,
    subtitle: gtk::Label,
}

impl DeviceRow {
    fn select(&self, selected: bool) {
        if selected {
            self.button.add_css_class("selected");
        } else {
            self.button.remove_css_class("selected");
        }
        self.check.set_opacity(if selected { 1.0 } else { 0.0 });
        self.subtitle.set_text(if selected {
            "Default device"
        } else {
            "Use this device"
        });
    }

    fn switching(&self, switching: bool) {
        if switching {
            self.button.add_css_class("switching");
            self.spinner.start();
            self.indicator.set_visible_child_name("spinner");
        } else {
            self.button.remove_css_class("switching");
            self.indicator.set_visible_child_name("check");
            self.spinner.stop();
        }
    }
}

impl View {
    fn connect_controls(self: &Rc<Self>) {
        for kind in [Kind::Output, Kind::Input] {
            let weak = Rc::downgrade(self);
            self.controls[kind.index()]
                .scale
                .connect_value_changed(move |scale| {
                    if let Some(view) = weak.upgrade() {
                        let control = &view.controls[kind.index()];
                        if control.updating.get() {
                            return;
                        }
                        let target = control.device.borrow().clone();
                        if let Some(device) = target {
                            let percent = scale.value().round() as u32;
                            control.label.set_text(&format!("{percent}%"));
                            view.request(Request::Volume(device, percent));
                        }
                    }
                });
            let weak = Rc::downgrade(self);
            self.controls[kind.index()].mute.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    let target = view.controls[kind.index()].device.borrow().clone();
                    if let Some(device) = target {
                        view.request(Request::Mute(device));
                    }
                }
            });
        }
    }

    fn request(&self, request: Request) {
        if self.busy.get() {
            if matches!(request, Request::Refresh) {
                return;
            }
            let mut queue = self.queue.borrow_mut();
            // Dragging can generate many events; only send the latest pending level.
            if let Request::Volume(target, _) = &request
                && let Some(queued) = queue.iter_mut().rev().find(|queued| matches!(queued, Request::Volume(device, _) if device.name == target.name && device.kind == target.kind))
            {
                *queued = request;
            } else {
                queue.push_back(request);
            }
            return;
        }
        self.busy.set(true);
        if let Request::Select(target) = &request {
            self.switch_started.set(Some(Instant::now()));
            self.message.set_visible(false);
            for row in self.rows.borrow().iter() {
                row.switching(row.device.name == target.name && row.device.kind == target.kind);
            }
        }
        if self.sender.send(request).is_err() {
            self.busy.set(false);
            self.switch_started.set(None);
            for row in self.rows.borrow().iter() {
                row.switching(false);
            }
            self.message
                .set_text("The audio worker stopped. Close and reopen the menu.");
            self.message.set_visible(true);
        }
    }

    fn update(self: &Rc<Self>, result: Result<Snapshot, String>) {
        self.busy.set(false);
        self.switch_started.set(None);
        match result {
            Err(error) => {
                self.message.set_text(&error);
                self.message.set_visible(true);
            }
            Ok(snapshot) => {
                self.message.set_visible(false);
                for kind in [Kind::Output, Kind::Input] {
                    let pending = self.queue.borrow().iter().any(|request| matches!(request, Request::Volume(device, _) if device.kind == kind));
                    self.controls[kind.index()].sync(&snapshot, kind, pending);
                }
                if self
                    .snapshot
                    .borrow()
                    .as_ref()
                    .is_some_and(|previous| previous.devices == snapshot.devices)
                {
                    // Keep row widgets alive so selection changes can crossfade in place.
                    for row in self.rows.borrow().iter() {
                        row.select(snapshot.is_default(&row.device));
                    }
                } else {
                    self.populate(&self.output, Kind::Output, &snapshot);
                    self.populate(&self.input, Kind::Input, &snapshot);
                }
                *self.snapshot.borrow_mut() = Some(snapshot);
            }
        }
        for row in self.rows.borrow().iter() {
            row.switching(false);
        }
        let next = self.queue.borrow_mut().pop_front();
        if let Some(next) = next {
            self.request(next);
        }
    }

    fn populate(self: &Rc<Self>, container: &gtk::Box, kind: Kind, snapshot: &Snapshot) {
        self.rows.borrow_mut().retain(|row| row.device.kind != kind);
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
        let devices: Vec<_> = snapshot.devices.iter().filter(|d| d.kind == kind).collect();
        if devices.is_empty() {
            let label = gtk::Label::new(Some(match kind {
                Kind::Output => "No output devices connected",
                Kind::Input => "No input devices connected",
            }));
            label.add_css_class("empty-state");
            container.append(&label);
        }
        for device in devices {
            let selected = snapshot.is_default(device);
            let button = gtk::Button::new();
            button.add_css_class("device");
            button.set_tooltip_text(Some(&device.name));
            if selected {
                button.add_css_class("selected");
            }
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let image = gtk::Image::from_icon_name(device.icon());
            image.set_pixel_size(20);
            row.append(&image);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
            text.set_hexpand(true);
            let label = gtk::Label::new(Some(&device.label));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(32);
            text.append(&label);
            let subtitle = gtk::Label::new(Some(if selected {
                "Default device"
            } else {
                "Use this device"
            }));
            subtitle.set_xalign(0.0);
            subtitle.add_css_class("device-subtitle");
            text.append(&subtitle);
            row.append(&text);
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_opacity(if selected { 1.0 } else { 0.0 });
            let indicator = gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .transition_duration(160)
                .valign(gtk::Align::Center)
                .build();
            let spinner = gtk::Spinner::new();
            spinner.set_size_request(20, 20);
            indicator.add_named(&check, Some("check"));
            indicator.add_named(&spinner, Some("spinner"));
            indicator.set_visible_child_name("check");
            row.append(&indicator);
            button.set_child(Some(&row));
            let view = Rc::downgrade(self);
            let target = device.clone();
            button.connect_clicked(move |_| {
                if let Some(view) = view.upgrade() {
                    view.request(Request::Select(target.clone()));
                }
            });
            container.append(&button);
            self.rows.borrow_mut().push(DeviceRow {
                device: device.clone(),
                button,
                indicator,
                spinner,
                check,
                subtitle,
            });
        }
    }
}

pub fn run(options: Options) -> Result<(), String> {
    // GTK arguments are parsed above; only pass the executable name to GApplication.
    let app = gtk::Application::builder()
        .application_id(if options.demo {
            "io.github.wpwidget.Demo"
        } else {
            "io.github.wpwidget.Audio"
        })
        .build();
    app.connect_activate(move |app| {
        if let Some(window) = app.windows().first() {
            window.close();
        } else {
            build(app, &options);
        }
    });
    let code = app.run_with_args(&["wpwidget"]);
    if code == glib::ExitCode::SUCCESS {
        Ok(())
    } else {
        Err("GTK application could not start".into())
    }
}

fn section(title: &str, kind: Kind, parent: &gtk::Box) -> (gtk::Box, VolumeControl) {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    header.add_css_class("section-heading");
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    header.append(&label);
    parent.append(&header);
    let control = VolumeControl::new(kind);
    parent.append(&control.container);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("device-list");
    parent.append(&list);
    (list, control)
}

fn build(app: &gtk::Application, options: &Options) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Audio Devices")
        .decorated(false)
        .resizable(false)
        .default_width(380)
        .build();
    window.add_css_class("audio-popup");
    let css = gtk::CssProvider::new();
    css.load_from_data(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &WidgetExt::display(&window),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("wpwidget"));
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.set_exclusive_zone(0);
        let vertical = if options.position.starts_with("top") {
            Edge::Top
        } else {
            Edge::Bottom
        };
        let horizontal = if options.position.ends_with("right") {
            Edge::Right
        } else {
            Edge::Left
        };
        window.set_anchor(vertical, true);
        window.set_anchor(horizontal, true);
        window.set_margin(vertical, options.margin);
        window.set_margin(horizontal, options.margin);
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("panel");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.add_css_class("panel-heading");
    let title = gtk::Label::new(Some("Sound"));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.add_css_class("title");
    header.append(&title);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("close");
    close.set_tooltip_text(Some("Close"));
    let weak_window = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak_window.upgrade() {
            window.close();
        }
    });
    header.append(&close);
    root.append(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let (output, output_control) = section("Output", Kind::Output, &content);
    let (input, input_control) = section("Input", Kind::Input, &content);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .max_content_height(520)
        .propagate_natural_height(true)
        .child(&content)
        .build();
    root.append(&scroll);
    let message = gtk::Label::new(Some("Finding audio devices…"));
    message.set_wrap(true);
    message.set_max_width_chars(40);
    message.add_css_class("message");
    root.append(&message);
    if options.demo {
        let footer = gtk::Label::new(Some("Preview · system audio is unchanged"));
        footer.add_css_class("footer");
        root.append(&footer);
    }
    window.set_child(Some(&root));

    let keys = gtk::EventControllerKey::new();
    let weak_window = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            if let Some(window) = weak_window.upgrade() {
                window.close();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    let (sender, requests) = mpsc::channel();
    let (results, receiver) = mpsc::channel();
    let demo = options.demo;
    std::thread::spawn(move || {
        let mut preview = audio::demo_snapshot();
        for request in requests {
            let result = if demo {
                match request {
                    Request::Select(device) => match device.kind {
                        Kind::Output => preview.output = Some(device.name),
                        Kind::Input => preview.input = Some(device.name),
                    },
                    Request::Volume(device, percent) => {
                        if let Some(volume) = &mut preview.volumes[device.kind.index()] {
                            volume.percent = percent;
                        }
                    }
                    Request::Mute(device) => {
                        if let Some(volume) = &mut preview.volumes[device.kind.index()] {
                            volume.muted = !volume.muted;
                        }
                    }
                    Request::Refresh => {}
                }
                Ok(preview.clone())
            } else {
                match request {
                    Request::Refresh => audio::snapshot(),
                    Request::Select(device) => audio::set_default(&device),
                    Request::Volume(device, percent) => audio::set_volume(&device, percent),
                    Request::Mute(device) => audio::toggle_mute(&device),
                }
            };
            if results.send(result).is_err() {
                break;
            }
        }
    });
    let view = Rc::new(View {
        output,
        input,
        message,
        busy: Cell::new(false),
        snapshot: RefCell::new(None),
        rows: RefCell::new(Vec::new()),
        switch_started: Cell::new(None),
        controls: [output_control, input_control],
        queue: RefCell::new(VecDeque::new()),
        sender,
    });
    view.connect_controls();
    view.request(Request::Refresh);
    let weak_window = window.downgrade();
    let weak_view = Rc::downgrade(&view);
    let mut refresh = Some(glib::timeout_add_local(Duration::from_secs(2), move || {
        if let Some(view) = weak_view.upgrade() {
            view.request(Request::Refresh);
            ControlFlow::Continue
        } else {
            ControlFlow::Break
        }
    }));
    let mut completed = None;
    glib::timeout_add_local(Duration::from_millis(16), move || {
        if weak_window.upgrade().is_none() {
            if let Some(refresh) = refresh.take() {
                refresh.remove();
            }
            return ControlFlow::Break;
        }
        if let Ok(result) = receiver.try_recv() {
            completed = Some(result);
        }
        // Keep a fast successful switch visible long enough for a short animation.
        if view
            .switch_started
            .get()
            .is_none_or(|start| start.elapsed() >= Duration::from_millis(250))
            && let Some(result) = completed.take()
        {
            view.update(result);
        }
        ControlFlow::Continue
    });
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a graphical session; run with --ignored --test-threads=1"]
    fn passive_refresh_keeps_icons_enabled_and_preserves_rows() {
        gtk::init().unwrap();
        let css_errors = Rc::new(RefCell::new(Vec::new()));
        let errors = Rc::clone(&css_errors);
        let css = gtk::CssProvider::new();
        css.connect_parsing_error(move |_, _, error| errors.borrow_mut().push(error.to_string()));
        css.load_from_data(include_str!("style.css"));
        assert!(css_errors.borrow().is_empty(), "{:?}", css_errors.borrow());
        let (sender, requests) = mpsc::channel();
        let view = Rc::new(View {
            output: gtk::Box::new(gtk::Orientation::Vertical, 0),
            input: gtk::Box::new(gtk::Orientation::Vertical, 0),
            message: gtk::Label::new(None),
            busy: Cell::new(false),
            snapshot: RefCell::new(None),
            rows: RefCell::new(Vec::new()),
            switch_started: Cell::new(None),
            controls: [
                VolumeControl::new(Kind::Output),
                VolumeControl::new(Kind::Input),
            ],
            queue: RefCell::new(VecDeque::new()),
            sender,
        });
        view.connect_controls();
        let snapshot = audio::demo_snapshot();
        view.update(Ok(snapshot.clone()));
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert_eq!(view.controls[0].scale.value(), 65.0);
        assert_eq!(view.controls[1].scale.value(), 80.0);
        let original_row = view.output.first_child().unwrap();
        view.request(Request::Refresh);
        assert!(matches!(requests.try_recv(), Ok(Request::Refresh)));
        assert!(view.output.is_sensitive());
        assert!(view.input.is_sensitive());
        view.update(Ok(snapshot.clone()));
        assert_eq!(view.output.first_child().unwrap(), original_row);
        view.request(Request::Select(snapshot.devices[0].clone()));
        assert!(matches!(requests.try_recv(), Ok(Request::Select(_))));
        assert!(view.output.is_sensitive());
        assert!(view.input.is_sensitive());
        assert!(view.rows.borrow()[0].spinner.is_spinning());
        assert!(!view.rows.borrow()[1].spinner.is_spinning());
        let mut after = snapshot;
        after.output = Some(after.devices[0].name.clone());
        view.update(Ok(after.clone()));
        assert_eq!(view.output.first_child().unwrap(), original_row);
        assert!(view.rows.borrow()[0].button.has_css_class("selected"));
        assert!(!view.rows.borrow()[1].button.has_css_class("selected"));
        assert!(!view.rows.borrow()[0].spinner.is_spinning());
        assert!(view.output.is_sensitive());
        assert!(view.input.is_sensitive());

        // Slider callbacks target the correct device and coalesce rapid dragging.
        view.controls[0].scale.set_value(30.0);
        assert!(
            matches!(requests.try_recv(), Ok(Request::Volume(device, 30)) if device.kind == Kind::Output)
        );
        view.controls[0].scale.set_value(35.0);
        view.controls[0].scale.set_value(40.0);
        assert_eq!(view.queue.borrow().len(), 1);
        after.volumes[0].as_mut().unwrap().percent = 30;
        view.update(Ok(after.clone()));
        assert_eq!(view.controls[0].scale.value(), 40.0);
        assert!(matches!(requests.try_recv(), Ok(Request::Volume(_, 40))));
        after.volumes[0].as_mut().unwrap().percent = 40;
        view.update(Ok(after.clone()));
        assert_eq!(view.controls[1].scale.value(), 80.0);
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        view.controls[1].mute.emit_clicked();
        assert!(
            matches!(requests.try_recv(), Ok(Request::Mute(device)) if device.kind == Kind::Input)
        );
        after.volumes[1].as_mut().unwrap().muted = true;
        view.update(Ok(after));
        assert!(view.controls[1].mute.has_css_class("muted"));
        assert!(!view.controls[0].mute.has_css_class("muted"));
        assert_eq!(view.controls[1].scale.value(), 80.0);
        assert!(matches!(
            requests.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }
}
