use adw::prelude::*;
use gtk4::{gdk, glib, prelude::*};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use super::markdown::markdown_to_pango;
use super::translation_phase::TranslationPhase;
use crate::client::{RecoveryHint, TranslationFailure};

const STREAM_BATCH_INTERVAL: Duration = Duration::from_millis(75);
const STREAM_BATCH_BYTES: usize = 16 * 1024;

struct StreamTasks {
    request: AbortHandle,
    ui: glib::JoinHandle<()>,
}

impl StreamTasks {
    fn cancel(&self) {
        self.request.abort();
        self.ui.abort();
    }
}

impl Drop for StreamTasks {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Default)]
struct ReadingPosition {
    following: Cell<bool>,
    changing_scroll: Cell<bool>,
}

type RetryAction = Rc<dyn Fn(&ResultPanel)>;
type RecoveryAction = Rc<dyn Fn(RecoveryHint)>;

#[derive(Default)]
struct PanelSession {
    generation: Cell<u64>,
    running: Cell<bool>,
    stopped: Cell<bool>,
    phase: Cell<TranslationPhase>,
    stopped_while_queued: Cell<bool>,
    closed: Cell<bool>,
    retry_dispatching: Cell<bool>,
    tasks: RefCell<Option<StreamTasks>>,
    retry: RefCell<Option<RetryAction>>,
    recovery: RefCell<Option<RecoveryAction>>,
    recovery_hint: Cell<Option<RecoveryHint>>,
}

impl PanelSession {
    fn close(&self) {
        self.closed.set(true);
        self.running.set(false);
        self.generation.set(self.generation.get().wrapping_add(1));
        self.tasks.borrow_mut().take();
        // The retry closure may own the screenshot. Release it at close, even
        // when another caller still holds a temporary ResultPanel clone.
        self.retry.borrow_mut().take();
        self.recovery.borrow_mut().take();
    }
}

#[derive(Clone)]
pub struct ResultPanel {
    window: adw::Window,
    title: adw::WindowTitle,
    view: gtk4::TextView,
    scrolled: gtk4::ScrolledWindow,
    reading: Rc<ReadingPosition>,
    status_label: gtk4::Label,
    spinner: gtk4::Spinner,
    copy_button: gtk4::Button,
    capture_button: gtk4::Button,
    settings_button: gtk4::Button,
    stop_button: gtk4::Button,
    retry_button: gtk4::Button,
    recovery_button: gtk4::Button,
    request_actions: gtk4::Box,
    copy_feedback: gtk4::Label,
    feedback_generation: Rc<Cell<u64>>,
    session: Rc<PanelSession>,
}

// Signal handlers hold only this weak view of the panel. The window owns the
// non-widget session separately, so its callbacks cannot keep the window alive.
struct WeakResultPanel {
    window: glib::WeakRef<adw::Window>,
    title: glib::WeakRef<adw::WindowTitle>,
    view: glib::WeakRef<gtk4::TextView>,
    scrolled: glib::WeakRef<gtk4::ScrolledWindow>,
    reading: Weak<ReadingPosition>,
    status_label: glib::WeakRef<gtk4::Label>,
    spinner: glib::WeakRef<gtk4::Spinner>,
    copy_button: glib::WeakRef<gtk4::Button>,
    capture_button: glib::WeakRef<gtk4::Button>,
    settings_button: glib::WeakRef<gtk4::Button>,
    stop_button: glib::WeakRef<gtk4::Button>,
    retry_button: glib::WeakRef<gtk4::Button>,
    recovery_button: glib::WeakRef<gtk4::Button>,
    request_actions: glib::WeakRef<gtk4::Box>,
    copy_feedback: glib::WeakRef<gtk4::Label>,
    feedback_generation: Weak<Cell<u64>>,
    session: Weak<PanelSession>,
}

impl WeakResultPanel {
    fn upgrade(&self) -> Option<ResultPanel> {
        Some(ResultPanel {
            window: self.window.upgrade()?,
            title: self.title.upgrade()?,
            view: self.view.upgrade()?,
            scrolled: self.scrolled.upgrade()?,
            reading: self.reading.upgrade()?,
            status_label: self.status_label.upgrade()?,
            spinner: self.spinner.upgrade()?,
            copy_button: self.copy_button.upgrade()?,
            capture_button: self.capture_button.upgrade()?,
            settings_button: self.settings_button.upgrade()?,
            stop_button: self.stop_button.upgrade()?,
            retry_button: self.retry_button.upgrade()?,
            recovery_button: self.recovery_button.upgrade()?,
            request_actions: self.request_actions.upgrade()?,
            copy_feedback: self.copy_feedback.upgrade()?,
            feedback_generation: self.feedback_generation.upgrade()?,
            session: self.session.upgrade()?,
        })
    }
}

impl ResultPanel {
    pub fn new(app: &adw::Application) -> Self {
        let window = adw::Window::builder()
            .application(app)
            .title("SnapTranslate · 译文")
            .default_width(520)
            .default_height(420)
            .width_request(360)
            .height_request(260)
            .resizable(true)
            .build();

        let header = adw::HeaderBar::new();
        header.set_decoration_layout(Some(":close"));
        let title = adw::WindowTitle::new("译文", "截图翻译");
        header.set_title_widget(Some(&title));

        let capture_button = gtk4::Button::with_label("再次截图");
        capture_button.set_tooltip_text(Some(
            "使用此窗口的目标语言再次截图；Ctrl+Shift+T 使用默认语言",
        ));
        capture_button.set_sensitive(false);
        let settings_button = gtk4::Button::with_label("设置");
        settings_button.set_tooltip_text(Some("打开设置（Ctrl+,）"));
        settings_button.set_sensitive(false);
        let copy_button = gtk4::Button::with_label("复制译文");
        copy_button.set_tooltip_text(Some("复制全部译文（Ctrl+Shift+C）；Ctrl+C 优先复制选区"));
        copy_button.set_sensitive(false);

        let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        toolbar.set_margin_top(6);
        toolbar.set_margin_bottom(6);
        toolbar.append(&capture_button);
        toolbar.append(&settings_button);
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);
        toolbar.append(&copy_button);

        let copy_feedback = gtk4::Label::builder()
            .label("")
            .css_classes(["dim-label"])
            .build();

        let scrolled = gtk4::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .build();
        // Builder accessibility markup also works with the project's GTK 4 baseline;
        // gtk-rs's typed accessibility setters require the newer v4_10 feature.
        let view: gtk4::TextView = gtk4::Builder::from_string(
            r#"<interface><object class="GtkTextView" id="translation">
              <accessibility><property name="label">译文内容</property></accessibility>
            </object></interface>"#,
        )
        .object("translation")
        .expect("static TextView definition is valid");
        view.set_editable(false);
        view.set_cursor_visible(true);
        view.set_wrap_mode(gtk4::WrapMode::WordChar);
        view.set_left_margin(16);
        view.set_right_margin(16);
        view.set_top_margin(12);
        view.set_bottom_margin(12);
        view.set_pixels_below_lines(4);
        view.set_hexpand(true);
        view.set_vexpand(true);
        scrolled.set_child(Some(&view));
        view.set_tooltip_text(Some(
            "选中文字后 Ctrl+C 复制；Ctrl+单击网址可在默认应用中打开",
        ));
        let open_link = gtk4::GestureClick::new();
        open_link.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak_view = view.downgrade();
        let link_feedback = copy_feedback.downgrade();
        open_link.connect_pressed(move |gesture, _, x, y| {
            if !gesture
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
            {
                return;
            }
            let Some(view) = weak_view.upgrade() else {
                return;
            };
            let (x, y) = view.window_to_buffer_coords(
                gtk4::TextWindowType::Text,
                x as i32,
                y as i32 + view.top_margin(),
            );
            let Some(iter) = view.iter_at_location(x, y) else {
                return;
            };
            let buffer = view.buffer();
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            if let Some(uri) = link_at_offset(&text, iter.offset() as usize) {
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                tracing::debug!("Opening user-selected link");
                if gtk4::gio::AppInfo::launch_default_for_uri(
                    &uri,
                    None::<&gtk4::gio::AppLaunchContext>,
                )
                .is_err()
                {
                    if let Some(feedback) = link_feedback.upgrade() {
                        feedback.set_label("无法打开链接，请复制网址。");
                    }
                }
            }
        });
        view.add_controller(open_link);

        let spinner = gtk4::Spinner::new();
        spinner.start();
        let status_label = gtk4::Label::builder()
            .label("正在翻译…")
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["dim-label"])
            .build();
        let status_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        status_row.set_margin_start(16);
        status_row.set_margin_end(16);
        status_row.set_margin_top(8);
        status_row.set_margin_bottom(8);
        status_row.append(&spinner);
        status_row.append(&status_label);
        status_row.append(&copy_feedback);

        let stop_button = gtk4::Button::with_label("停止翻译");
        let retry_button = gtk4::Button::with_label("原图重试");
        retry_button.set_tooltip_text(Some(
            "使用当前已保存的接口设置重新翻译同一截图，目标语言不变",
        ));
        let recovery_button = gtk4::Button::new();
        let request_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        request_actions.set_margin_start(12);
        request_actions.set_margin_end(12);
        request_actions.set_margin_bottom(8);
        request_actions.append(&stop_button);
        request_actions.append(&retry_button);
        request_actions.append(&recovery_button);
        request_actions.set_visible(false);
        stop_button.set_visible(false);
        retry_button.set_visible(false);
        recovery_button.set_visible(false);

        let bottom_button = gtk4::Button::builder()
            .label("回到底部")
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::End)
            .margin_bottom(12)
            .css_classes(["pill"])
            .visible(false)
            .build();
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&scrolled));
        overlay.add_overlay(&bottom_button);

        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.append(&header);
        root.append(&toolbar);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&overlay);
        root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        root.append(&status_row);
        root.append(&request_actions);
        window.set_content(Some(&root));

        let reading = Rc::new(ReadingPosition::default());
        reading.following.set(true);
        connect_scrolling(&view, &scrolled, &bottom_button, &reading);

        // Copy from the displayed buffer: rendered Markdown is plain text on the clipboard.
        let buffer = view.buffer().downgrade();
        let feedback = copy_feedback.downgrade();
        let feedback_generation = Rc::new(Cell::new(0_u64));
        let button_generation = feedback_generation.clone();
        copy_button.connect_clicked(move |_| {
            if let (Some(buffer), Some(feedback)) = (buffer.upgrade(), feedback.upgrade()) {
                copy_displayed_text(&buffer, &feedback, &button_generation, false);
            }
        });

        let key_controller = gtk4::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let weak_window = window.downgrade();
        let buffer = view.buffer().downgrade();
        let feedback = copy_feedback.downgrade();
        let key_reading = reading.clone();
        let key_feedback_generation = feedback_generation.clone();
        key_controller.connect_key_pressed(move |_, key, _, modifiers| {
            let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            if key == gdk::Key::Escape || (control && (key == gdk::Key::w || key == gdk::Key::W)) {
                if let Some(window) = weak_window.upgrade() {
                    window.close();
                }
                return glib::Propagation::Stop;
            }
            if control && (key == gdk::Key::c || key == gdk::Key::C) {
                if let (Some(buffer), Some(feedback)) = (buffer.upgrade(), feedback.upgrade()) {
                    let selection_first = !modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                    copy_displayed_text(
                        &buffer,
                        &feedback,
                        &key_feedback_generation,
                        selection_first,
                    );
                }
                return glib::Propagation::Stop;
            }
            if matches!(key, gdk::Key::Up | gdk::Key::Page_Up | gdk::Key::Home) {
                key_reading.following.set(false);
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key_controller);

        let panel = Self {
            window,
            title,
            view,
            scrolled,
            reading,
            status_label,
            spinner,
            copy_button,
            capture_button,
            settings_button,
            stop_button,
            retry_button,
            recovery_button,
            request_actions,
            copy_feedback,
            feedback_generation,
            session: Rc::new(PanelSession::default()),
        };
        let weak = panel.downgrade();
        panel.stop_button.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.stop_translation();
            }
        });
        let weak = panel.downgrade();
        panel.retry_button.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                if panel.session.closed.get()
                    || panel.is_running()
                    || panel.session.retry_dispatching.replace(true)
                {
                    return;
                }
                let action = panel.session.retry.borrow().clone();
                panel.refresh_request_actions();
                if let Some(action) = action {
                    action(&panel);
                }
                panel.session.retry_dispatching.set(false);
                panel.refresh_request_actions();
            }
        });
        let weak = panel.downgrade();
        panel.recovery_button.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                let action = panel.session.recovery.borrow().clone();
                if let (Some(action), Some(hint)) = (action, panel.session.recovery_hint.get()) {
                    action(hint);
                }
            }
        });
        let session = panel.session.clone();
        panel.window.connect_close_request(move |_| {
            session.close();
            glib::Propagation::Proceed
        });
        let session = panel.session.clone();
        panel.window.connect_destroy(move |_| session.close());
        panel
    }

    fn downgrade(&self) -> WeakResultPanel {
        WeakResultPanel {
            window: self.window.downgrade(),
            title: self.title.downgrade(),
            view: self.view.downgrade(),
            scrolled: self.scrolled.downgrade(),
            reading: Rc::downgrade(&self.reading),
            status_label: self.status_label.downgrade(),
            spinner: self.spinner.downgrade(),
            copy_button: self.copy_button.downgrade(),
            capture_button: self.capture_button.downgrade(),
            settings_button: self.settings_button.downgrade(),
            stop_button: self.stop_button.downgrade(),
            retry_button: self.retry_button.downgrade(),
            recovery_button: self.recovery_button.downgrade(),
            request_actions: self.request_actions.downgrade(),
            copy_feedback: self.copy_feedback.downgrade(),
            feedback_generation: Rc::downgrade(&self.feedback_generation),
            session: Rc::downgrade(&self.session),
        }
    }

    pub fn set_retry_action(&self, action: Rc<dyn Fn(&ResultPanel)>) {
        if !self.session.closed.get() {
            *self.session.retry.borrow_mut() = Some(action);
            self.refresh_request_actions();
        }
    }

    pub fn set_recovery_action(&self, action: Rc<dyn Fn(RecoveryHint)>) {
        if !self.session.closed.get() {
            *self.session.recovery.borrow_mut() = Some(action);
            self.refresh_request_actions();
        }
    }

    pub fn is_running(&self) -> bool {
        self.session.running.get()
    }

    fn refresh_request_actions(&self) {
        let running = self.is_running() && !self.session.closed.get();
        self.stop_button.set_visible(running);
        self.stop_button.set_label(match self.session.phase.get() {
            TranslationPhase::Queued => "取消等待",
            TranslationPhase::Running => "停止翻译",
        });
        self.stop_button
            .set_sensitive(running && !self.session.stopped.get());
        let retry = !running && !self.session.closed.get() && self.session.retry.borrow().is_some();
        self.retry_button.set_visible(retry);
        self.retry_button
            .set_sensitive(retry && !self.session.retry_dispatching.get());
        let recovery_label = if running || self.session.recovery.borrow().is_none() {
            None
        } else {
            match self.session.recovery_hint.get() {
                Some(RecoveryHint::Credentials) => Some("检查 API Key"),
                Some(RecoveryHint::Service) => Some("检查模型与接口"),
                _ => None,
            }
        };
        if let Some(label) = recovery_label {
            self.recovery_button.set_label(label);
        }
        self.recovery_button.set_visible(recovery_label.is_some());
        self.request_actions
            .set_visible(running || retry || recovery_label.is_some());
    }

    fn stop_translation(&self) {
        if !self.is_running() || self.session.stopped.replace(true) {
            return;
        }
        let queued = self.session.phase.get() == TranslationPhase::Queued;
        self.session.stopped_while_queued.set(queued);
        if let Some(tasks) = self.session.tasks.borrow().as_ref() {
            // Keep the UI task alive to drain queued text and render the partial result.
            tasks.request.abort();
        }
        self.status_label.set_label(if queued {
            "正在取消等待…"
        } else {
            "正在停止翻译…"
        });
        self.refresh_request_actions();
    }

    fn update_translation_phase(&self, phase: TranslationPhase) {
        if self.session.closed.get() || !self.is_running() || self.session.stopped.get() {
            return;
        }
        self.session.phase.set(phase);
        self.status_label.set_label(match phase {
            TranslationPhase::Queued => "等待翻译，前面有任务正在处理…",
            TranslationPhase::Running => "正在翻译…",
        });
        self.refresh_request_actions();
    }

    pub fn set_actions(&self, on_capture: Rc<dyn Fn()>, on_settings: Rc<dyn Fn()>) {
        self.capture_button.connect_clicked(move |_| on_capture());
        self.settings_button.connect_clicked(move |_| on_settings());
        self.capture_button.set_sensitive(true);
        self.settings_button.set_sensitive(true);
    }

    pub fn set_context(&self, sequence: u64, language: &str) {
        let title = format!("译文 {sequence}");
        self.window
            .set_title(Some(&format!("SnapTranslate · {title}")));
        self.title.set_title(&title);
        self.title.set_subtitle(language_name(language));
    }

    pub fn set_activation_token(&self, token: &str) {
        self.window.set_startup_id(token);
    }

    pub fn show(&self) {
        self.window.present();
        self.view.grab_focus();
    }

    pub fn close(&self) {
        self.window.close();
    }

    /// Replace this window's request. Old tasks and deferred UI work are invalidated.
    pub fn listen_stream(
        &self,
        rx: mpsc::Receiver<String>,
        completion: oneshot::Receiver<Result<(), TranslationFailure>>,
        request_abort: AbortHandle,
    ) {
        let (_, phase) = watch::channel(TranslationPhase::Running);
        self.listen_stream_with_state(rx, completion, request_abort, phase);
    }

    /// Observe queue admission without treating waiting time as an active translation.
    pub fn listen_stream_with_state(
        &self,
        mut rx: mpsc::Receiver<String>,
        completion: oneshot::Receiver<Result<(), TranslationFailure>>,
        request_abort: AbortHandle,
        mut phase: watch::Receiver<TranslationPhase>,
    ) {
        if self.session.closed.get() {
            request_abort.abort();
            return;
        }
        let previous = self.session.tasks.borrow_mut().take();
        drop(previous);
        let generation = self.session.generation.get().wrapping_add(1);
        self.session.generation.set(generation);
        self.session.running.set(true);
        self.session.stopped.set(false);
        self.session.stopped_while_queued.set(false);
        self.session.recovery_hint.set(None);
        self.reading.following.set(true);
        self.reading.changing_scroll.set(true);
        self.view.buffer().set_text("");
        self.reading.changing_scroll.set(false);
        self.copy_button.set_sensitive(false);
        self.copy_feedback.set_label("");
        self.feedback_generation
            .set(self.feedback_generation.get().wrapping_add(1));
        self.status_label.remove_css_class("error");
        self.status_label.add_css_class("dim-label");
        self.update_translation_phase(*phase.borrow_and_update());
        self.spinner.set_visible(true);
        self.spinner.start();
        self.refresh_request_actions();
        let weak = self.downgrade();

        let ui_task = glib::MainContext::default().spawn_local(async move {
            let mut raw_text = String::new();
            let mut stream_closed = false;
            let mut has_visible_text = false;
            let mut phase_open = true;
            loop {
                let first = loop {
                    tokio::select! {
                        biased;
                        changed = phase.changed(), if phase_open => {
                            if changed.is_err() {
                                // A completed/dropped watch sender must not create a busy loop.
                                phase_open = false;
                                continue;
                            }
                            let current_phase = *phase.borrow_and_update();
                            let Some(panel) = weak.upgrade() else { return };
                            if !panel.accepts_generation(generation) { return; }
                            panel.update_translation_phase(current_phase);
                        }
                        delta = rx.recv() => break delta,
                    }
                };
                let Some(first) = first else { break };
                let mut batch = first;
                let delay = glib::timeout_future(STREAM_BATCH_INTERVAL);
                tokio::pin!(delay);
                while batch.len() < STREAM_BATCH_BYTES {
                    tokio::select! {
                        _ = &mut delay => break,
                        delta = rx.recv() => match delta {
                            Some(delta) => batch.push_str(&delta),
                            None => { stream_closed = true; break; }
                        }
                    }
                }
                let Some(panel) = weak.upgrade() else { return };
                if !panel.accepts_generation(generation) {
                    return;
                }
                raw_text.push_str(&batch);
                has_visible_text |= !batch.trim().is_empty();
                let buffer = panel.view.buffer();
                buffer.insert(&mut buffer.end_iter(), &batch);
                panel.copy_button.set_sensitive(has_visible_text);
                if panel.reading.following.get() {
                    scroll_to_bottom(&panel.scrolled.vadjustment(), &panel.reading);
                }
                if stream_closed {
                    break;
                }
            }

            let outcome = completion.await.unwrap_or_else(|_| {
                Err(TranslationFailure {
                    message: "翻译请求意外中断，请重试。".to_string(),
                    recovery: RecoveryHint::Retry,
                })
            });
            let Some(panel) = weak.upgrade() else { return };
            if !panel.accepts_generation(generation) {
                return;
            }
            let buffer = panel.view.buffer();
            let following = panel.reading.following.get();
            let previous_scroll = panel.scrolled.vadjustment().value();
            panel.reading.changing_scroll.set(true);
            buffer.set_text("");
            buffer.insert_markup(&mut buffer.start_iter(), &markdown_to_pango(&raw_text));
            panel.reading.changing_scroll.set(false);
            let has_text = !buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .trim()
                .is_empty();
            panel.copy_button.set_sensitive(has_text);

            // A previous request's idle callback must not move a new request's viewport.
            let weak_adjustment = panel.scrolled.vadjustment().downgrade();
            let weak_session = Rc::downgrade(&panel.session);
            let restore_reading = panel.reading.clone();
            glib::idle_add_local_once(move || {
                let Some(session) = weak_session.upgrade() else {
                    return;
                };
                if session.closed.get() || session.generation.get() != generation {
                    return;
                }
                if let Some(adjustment) = weak_adjustment.upgrade() {
                    restore_reading.following.set(following);
                    if following {
                        scroll_to_bottom(&adjustment, &restore_reading);
                    } else {
                        restore_reading.changing_scroll.set(true);
                        adjustment.set_value(previous_scroll.min(bottom_value(&adjustment)));
                        restore_reading.changing_scroll.set(false);
                    }
                }
            });

            panel.session.running.set(false);
            panel.spinner.stop();
            panel.spinner.set_visible(false);
            if panel.session.stopped.get() {
                panel
                    .status_label
                    .set_label(if panel.session.stopped_while_queued.get() {
                        "已取消等待。"
                    } else if has_text {
                        "已停止翻译，已保留收到的部分译文。"
                    } else {
                        "已停止翻译，未收到译文。"
                    });
                panel.refresh_request_actions();
            } else {
                match outcome {
                    Ok(()) if has_text => {
                        panel.status_label.set_label("翻译完成");
                        panel.refresh_request_actions();
                    }
                    Ok(()) => panel.set_failure(&TranslationFailure {
                        message: "未收到译文，请重新截屏后重试。".to_string(),
                        recovery: RecoveryHint::Recapture,
                    }),
                    Err(failure) => panel.set_failure(&failure),
                }
            }
        });
        *self.session.tasks.borrow_mut() = Some(StreamTasks {
            request: request_abort,
            ui: ui_task,
        });
    }

    fn accepts_generation(&self, generation: u64) -> bool {
        !self.session.closed.get() && self.session.generation.get() == generation
    }

    /// Show a request/preflight failure without discarding the displayed translation.
    pub fn set_failure(&self, failure: &TranslationFailure) {
        if self.session.closed.get() {
            return;
        }
        if self.is_running() {
            self.session
                .generation
                .set(self.session.generation.get().wrapping_add(1));
            let previous = self.session.tasks.borrow_mut().take();
            drop(previous);
            self.session.running.set(false);
        }
        self.spinner.stop();
        self.spinner.set_visible(false);
        self.session.recovery_hint.set(Some(failure.recovery));
        let buffer = self.view.buffer();
        let has_text = !buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .trim()
            .is_empty();
        self.status_label.set_label(&if has_text {
            format!("错误: {}\n已保留当前译文。", failure.message)
        } else {
            format!("错误: {}", failure.message)
        });
        self.status_label.remove_css_class("dim-label");
        self.status_label.add_css_class("error");
        self.refresh_request_actions();
    }

    pub fn set_error(&self, message: &str) {
        self.spinner.stop();
        self.spinner.set_visible(false);
        self.status_label.set_label(&format!("错误: {message}"));
        self.status_label.remove_css_class("dim-label");
        self.status_label.add_css_class("error");
    }
}

fn link_at_offset(text: &str, offset: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if offset >= chars.len() {
        return None;
    }
    let boundary = |c: char| c.is_whitespace() || "()<>\"'，。；]".contains(c);
    let start = (0..=offset)
        .rev()
        .find(|&i| boundary(chars[i]))
        .map_or(0, |i| i + 1);
    let end = (offset..chars.len())
        .find(|&i| boundary(chars[i]))
        .unwrap_or(chars.len());
    if start >= end {
        return None;
    }
    let value: String = chars[start..end].iter().collect();
    let url = reqwest::Url::parse(&value).ok()?;
    matches!(url.scheme(), "http" | "https" | "mailto").then_some(value)
}

fn language_name(language: &str) -> &str {
    match language {
        "" => "截图翻译",
        "zh-CN" | "zh-Hans" => "简体中文",
        "zh-TW" | "zh-Hant" => "繁體中文",
        "en" => "英语",
        "ja" => "日语",
        "ko" => "韩语",
        "fr" => "法语",
        "de" => "德语",
        "es" => "西班牙语",
        "ru" => "俄语",
        _ => language,
    }
}

fn copy_displayed_text(
    buffer: &gtk4::TextBuffer,
    feedback: &gtk4::Label,
    feedback_generation: &Rc<Cell<u64>>,
    selection_first: bool,
) {
    let bounds = if selection_first {
        buffer.selection_bounds()
    } else {
        None
    };
    let (start, end) = bounds.unwrap_or_else(|| (buffer.start_iter(), buffer.end_iter()));
    let text = buffer.text(&start, &end, false);
    if !text.trim().is_empty() {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&text);
            feedback.set_label("已复制");
            let generation = feedback_generation.get().wrapping_add(1);
            feedback_generation.set(generation);
            let feedback_generation = feedback_generation.clone();
            let feedback = feedback.downgrade();
            glib::timeout_add_local_once(Duration::from_secs(2), move || {
                if feedback_generation.get() == generation {
                    if let Some(feedback) = feedback.upgrade() {
                        feedback.set_label("");
                    }
                }
            });
        }
    }
}

fn bottom_value(adjustment: &gtk4::Adjustment) -> f64 {
    (adjustment.upper() - adjustment.page_size()).max(adjustment.lower())
}

fn scroll_to_bottom(adjustment: &gtk4::Adjustment, reading: &ReadingPosition) {
    reading.changing_scroll.set(true);
    adjustment.set_value(bottom_value(adjustment));
    reading.changing_scroll.set(false);
}

fn connect_scrolling(
    view: &gtk4::TextView,
    scrolled: &gtk4::ScrolledWindow,
    bottom_button: &gtk4::Button,
    reading: &Rc<ReadingPosition>,
) {
    let adjustment = scrolled.vadjustment();
    let weak_bottom = bottom_button.downgrade();
    let value_reading = reading.clone();
    adjustment.connect_value_changed(move |adjustment| {
        // Layout and cursor anchoring also change the adjustment. Only explicit
        // reader interaction below disables following; native relayout must not.
        if !value_reading.changing_scroll.get() {
            if value_reading.following.get() {
                scroll_to_bottom(adjustment, &value_reading);
            } else if bottom_value(adjustment) > adjustment.lower()
                && bottom_value(adjustment) - adjustment.value() <= 1.0
            {
                value_reading.following.set(true);
            }
        }
        if let Some(button) = weak_bottom.upgrade() {
            button.set_visible(!value_reading.following.get() && bottom_value(adjustment) > 0.0);
        }
    });
    let changed_reading = reading.clone();
    let weak_bottom = bottom_button.downgrade();
    adjustment.connect_changed(move |adjustment| {
        if changed_reading.following.get() {
            scroll_to_bottom(adjustment, &changed_reading);
        }
        if let Some(button) = weak_bottom.upgrade() {
            button.set_visible(!changed_reading.following.get() && bottom_value(adjustment) > 0.0);
        }
    });

    let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let scroll_reading = reading.clone();
    scroll.connect_scroll(move |_, _, dy| {
        if dy < 0.0 {
            scroll_reading.following.set(false);
        }
        glib::Propagation::Proceed
    });
    scrolled.add_controller(scroll);

    // Scrollbar dragging and selecting text are deliberate reading actions too.
    let scrollbar_pointer = gtk4::GestureClick::new();
    scrollbar_pointer.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let scrollbar_reading = reading.clone();
    scrollbar_pointer.connect_pressed(move |_, _, _, _| {
        scrollbar_reading.following.set(false);
    });
    scrolled.vscrollbar().add_controller(scrollbar_pointer);
    let selection_drag = gtk4::GestureDrag::new();
    selection_drag.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let selection_reading = reading.clone();
    selection_drag.connect_drag_begin(move |_, _, _| {
        selection_reading.following.set(false);
    });
    view.add_controller(selection_drag);

    let weak_adjustment = adjustment.downgrade();
    let button_reading = reading.clone();
    bottom_button.connect_clicked(move |button| {
        button_reading.following.set(true);
        if let Some(adjustment) = weak_adjustment.upgrade() {
            scroll_to_bottom(&adjustment, &button_reading);
        }
        button.set_visible(false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_session_releases_the_screenshot_owned_by_retry() {
        let session = PanelSession::default();
        let screenshot = Rc::new(vec![1_u8, 2, 3]);
        let screenshot_weak = Rc::downgrade(&screenshot);
        *session.retry.borrow_mut() = Some(Rc::new(move |_| {
            assert!(!screenshot.is_empty());
        }));
        session.running.set(true);
        session.generation.set(7);

        session.close();

        assert!(screenshot_weak.upgrade().is_none());
        assert!(!session.running.get());
        assert!(session.closed.get());
        assert_ne!(session.generation.get(), 7);
        session.close(); // The close-request and destroy signals may both run.
    }
    #[test]
    fn links_use_character_offsets_and_allow_only_user_openable_schemes() {
        assert_eq!(
            link_at_offset("中文 (https://example.com/path)", 10).as_deref(),
            Some("https://example.com/path")
        );
        assert!(link_at_offset("javascript:alert(1)", 3).is_none());
        assert!(link_at_offset("file:///etc/passwd", 8).is_none());
    }
}
