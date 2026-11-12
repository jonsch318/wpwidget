use crate::Options;
use crate::audio::{self, Device, Kind, Snapshot};
use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

enum Request {
    Refresh,
    Select(Device),
}

struct View {
    output: gtk::Box,
    input: gtk::Box,
    message: gtk::Label,
    busy: Cell<bool>,
    snapshot: RefCell<Option<Snapshot>>,
    rows: RefCell<Vec<DeviceRow>>,
    switch_started: Cell<Option<Instant>>,
    sender: Sender<Request>,
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
    fn request(&self, request: Request) {
        if self.busy.replace(true) {
            return;
        }
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
            let icon = match kind {
                Kind::Output => "audio-speakers-symbolic",
                Kind::Input => "audio-input-microphone-symbolic",
            };
            let image = gtk::Image::from_icon_name(icon);
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
            "io.github.wireplumberwidget.Demo"
        } else {
            "io.github.wireplumberwidget.Audio"
        })
        .build();
    app.connect_activate(move |app| {
        if let Some(window) = app.windows().first() {
            window.close();
        } else {
            build(app, &options);
        }
    });
    let code = app.run_with_args(&["wireplumber-widget"]);
    if code == glib::ExitCode::SUCCESS {
        Ok(())
    } else {
        Err("GTK application could not start".into())
    }
}

fn section(title: &str, icon: &str, parent: &gtk::Box) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 9);
    header.add_css_class("section-heading");
    header.append(&gtk::Image::from_icon_name(icon));
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    header.append(&label);
    parent.append(&header);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("device-list");
    parent.append(&list);
    list
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
        window.set_namespace(Some("wireplumber-widget"));
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
    let output = section("Sound Output", "audio-volume-high-symbolic", &content);
    let input = section("Sound Input", "audio-input-microphone-symbolic", &content);
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
                if let Request::Select(device) = request {
                    match device.kind {
                        Kind::Output => preview.output = Some(device.name),
                        Kind::Input => preview.input = Some(device.name),
                    }
                }
                Ok(preview.clone())
            } else {
                match request {
                    Request::Refresh => audio::snapshot(),
                    Request::Select(device) => audio::set_default(&device),
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
        sender,
    });
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
            sender,
        });
        let snapshot = audio::demo_snapshot();
        view.update(Ok(snapshot.clone()));
        let original_row = view.output.first_child().unwrap();
        view.request(Request::Refresh);
        assert!(matches!(requests.try_recv(), Ok(Request::Refresh)));
        assert!(view.output.is_sensitive());
        assert!(view.input.is_sensitive());
        view.update(Ok(snapshot.clone()));
        assert_eq!(view.output.first_child().unwrap(), original_row);
        view.request(Request::Select(snapshot.devices[0].clone()));
        assert!(view.output.is_sensitive());
        assert!(view.input.is_sensitive());
        assert!(view.rows.borrow()[0].spinner.is_spinning());
        assert!(!view.rows.borrow()[1].spinner.is_spinning());
        let mut after = snapshot;
        after.output = Some(after.devices[0].name.clone());
        view.update(Ok(after));
        assert_eq!(view.output.first_child().unwrap(), original_row);
        assert!(view.rows.borrow()[0].button.has_css_class("selected"));
        assert!(!view.rows.borrow()[1].button.has_css_class("selected"));
        assert!(!view.rows.borrow()[0].spinner.is_spinning());
        assert!(view.output.is_sensitive());
        assert!(view.input.is_sensitive());
    }
}
