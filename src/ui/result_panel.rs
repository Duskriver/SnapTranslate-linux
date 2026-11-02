use std::cell::RefCell;
use std::rc::Rc;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;
use tokio::sync::mpsc;
use tracing::info;

use super::markdown::markdown_to_pango;

pub struct ResultPanel {
    window: adw::Window,
    raw_text: Rc<RefCell<String>>,
    content_label: gtk4::Label,
    status_label: gtk4::Label,
}

impl ResultPanel {
    pub fn new(app: &adw::Application) -> Self {
        let window = adw::Window::builder()
            .application(app)
            .title("SnapTranslate · 译文")
            .default_width(460)
            .default_height(340)
            .resizable(true)
            .build();

        // Header bar with Copy and Close buttons
        let header = adw::HeaderBar::new();
        
        let copy_button = gtk4::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("复制译文")
            .build();

        let raw_text = Rc::new(RefCell::new(String::new()));
        let raw_text_clone = raw_text.clone();
        copy_button.connect_clicked(move |_| {
            if let Some(display) = gdk::Display::default() {
                let text = raw_text_clone.borrow().clone();
                if !text.is_empty() {
                    display.clipboard().set_text(&text);
                    info!("Copied translation to clipboard");
                }
            }
        });
        header.pack_start(&copy_button);

        // Content area
        let root_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root_box.append(&header);

        let scrolled = gtk4::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .build();

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        content_box.set_margin_start(16);
        content_box.set_margin_end(16);
        content_box.set_margin_top(12);
        content_box.set_margin_bottom(12);

        let status_label = gtk4::Label::builder()
            .label("正在翻译...")
            .halign(gtk4::Align::Start)
            .css_classes(["dim-label"])
            .build();
        content_box.append(&status_label);

        let content_label = gtk4::Label::builder()
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .yalign(0.0)
            .use_markup(true)
            .label("")
            .build();
        content_box.append(&content_label);

        scrolled.set_child(Some(&content_box));
        root_box.append(&scrolled);

        window.set_content(Some(&root_box));

        // Close on Esc key
        let key_controller = gtk4::EventControllerKey::new();
        let win_weak = window.downgrade();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(win) = win_weak.upgrade() {
                    win.close();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key_controller);

        Self {
            window,
            raw_text,
            content_label,
            status_label,
        }
    }

    pub fn show(&self) {
        self.window.present();
    }

    pub fn close(&self) {
        self.window.close();
    }

    /// Connects to a stream of translation tokens and updates the panel incrementally.
    pub fn listen_stream(&self, mut rx: mpsc::Receiver<String>) {
        let raw_text = self.raw_text.clone();
        let content_label = self.content_label.clone();
        let status_label = self.status_label.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Some(delta) = rx.recv().await {
                raw_text.borrow_mut().push_str(&delta);
                let current_md = raw_text.borrow().clone();
                let pango_text = markdown_to_pango(&current_md);
                content_label.set_markup(&pango_text);
            }
            status_label.set_visible(false);
        });
    }

    pub fn set_error(&self, message: &str) {
        self.status_label.set_label(&format!("错误: {}", message));
        self.status_label.set_visible(true);
    }
}
