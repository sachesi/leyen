//! The log window: what the daemon captured from games and its own operations,
//! for every game or one, following new lines as they arrive.

use std::cell::{Cell, OnceCell, RefCell};
use std::fmt::Write as _;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_ipc::LogEntry;
use leyen_model::i18n::gettext;
use leyen_model::models::LibraryItem;
use libadwaita as adw;

use crate::daemon::{self, DaemonEvent};
use crate::window::LeyenWindow;

/// Upper bound on lines kept in the view (the daemon ring is 1000).
const MAX_VIEW_LINES: i32 = 5000;

/// What to fetch from the daemon: everything again, or what came after the last pull.
#[derive(Clone, Copy)]
pub enum Pull {
    Full,
    Incremental,
}

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/log_window.ui")]
    pub struct LogWindow {
        #[template_child]
        pub filter_dropdown: TemplateChild<gtk4::DropDown>,
        #[template_child]
        pub stack: TemplateChild<gtk4::Stack>,
        #[template_child]
        pub scrolled_window: TemplateChild<gtk4::ScrolledWindow>,
        #[template_child]
        pub text_view: TemplateChild<gtk4::TextView>,
        /// The game each entry of the filter stands for; `None` is "All Logs".
        pub filter_ids: RefCell<Vec<Option<String>>>,
        /// Follow new lines, unless the view was scrolled up.
        pub follow: Cell<bool>,
        /// Value, upper bound and page size of the scroll position last seen.
        pub last_position: Cell<(f64, f64, f64)>,
        pub end_mark: OnceCell<gtk4::TextMark>,
        pub pulls: OnceCell<async_channel::Sender<Pull>>,
        pub tasks: RefCell<Vec<glib::JoinHandle<()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LogWindow {
        const NAME: &'static str = "LeyenLogWindow";
        type Type = super::LogWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LogWindow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }

        fn dispose(&self) {
            for task in self.tasks.take() {
                task.abort();
            }
        }
    }

    impl WidgetImpl for LogWindow {}

    impl WindowImpl for LogWindow {
        fn close_request(&self) -> glib::Propagation {
            for task in self.tasks.take() {
                task.abort();
            }
            self.parent_close_request()
        }
    }

    impl AdwWindowImpl for LogWindow {}

    #[gtk4::template_callbacks]
    impl LogWindow {
        #[template_callback]
        fn on_filter_changed(&self, _pspec: &glib::ParamSpec, _dropdown: &gtk4::DropDown) {
            self.follow.set(true);
            self.obj().request(Pull::Full);
        }

        #[template_callback]
        fn on_clear(&self, _button: &gtk4::Button) {
            let obj = self.obj().clone();
            glib::spawn_future_local(async move {
                daemon::clear_logs().await;
                obj.imp().follow.set(true);
                obj.request(Pull::Full);
            });
        }
    }
}

glib::wrapper! {
    pub struct LogWindow(ObjectSubclass<imp::LogWindow>)
        @extends adw::Window, gtk4::Window, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native,
                    gtk4::Root, gtk4::ShortcutManager;
}

impl LogWindow {
    /// Shows the log window, filtered to `game_id` when given. There is one.
    pub async fn present(parent: &LeyenWindow, game_id: Option<&str>) {
        let existing = parent.application().and_then(|app| {
            app.windows()
                .into_iter()
                .find_map(|window| window.downcast::<Self>().ok())
        });
        if let Some(window) = existing {
            if game_id.is_some() {
                window.select_game(game_id);
            }
            window.present();
            return;
        }

        let window: Self = glib::Object::builder()
            .property("application", parent.application())
            .property("transient-for", parent)
            .property("destroy-with-parent", true)
            .build();
        let library = daemon::load_library().await.unwrap_or_default();
        window.fill_filter(&library);
        window.select_game(game_id);
        window.present();
    }

    fn setup(&self) {
        let imp = self.imp();
        imp.follow.set(true);
        let buffer = imp.text_view.buffer();
        let _ = imp
            .end_mark
            .set(buffer.create_mark(Some("end"), &buffer.end_iter(), false));

        // Reaching the end follows it again; only moving up leaves it. The view also
        // moves by itself: on its way to the end, which changes nothing, and when it
        // is resized or drops old lines, which is not the user leaving.
        let adjustment = imp.scrolled_window.vadjustment();
        adjustment.connect_value_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |adjustment| {
                let imp = window.imp();
                let (value, upper, page) = (
                    adjustment.value(),
                    adjustment.upper(),
                    adjustment.page_size(),
                );
                let (last_value, last_upper, last_page) = imp.last_position.get();
                imp.last_position.set((value, upper, page));
                if value + page >= upper - 8.0 {
                    imp.follow.set(true);
                } else if value < last_value && upper >= last_upper && page == last_page {
                    imp.follow.set(false);
                }
            }
        ));
        // New lines are laid out after they are added, a few at a time; each time the
        // view grows or is resized, the end stays in sight.
        for property in ["upper", "page-size"] {
            adjustment.connect_notify_local(
                Some(property),
                glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |_, _| window.stick_to_end()
                ),
            );
        }

        // Every pull goes through one worker so a full rebuild and an incremental
        // append can never overlap: two in-flight pulls used to append the same
        // lines twice and move the offset backwards. The worker owns the offset.
        let (sender, receiver) = async_channel::unbounded::<Pull>();
        let _ = imp.pulls.set(sender);
        let worker = glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let mut offset = 0u64;
                while let Ok(pull) = receiver.recv().await {
                    // Collapse a queued burst into its strongest request.
                    let mut pull = pull;
                    while let Ok(next) = receiver.try_recv() {
                        if matches!(next, Pull::Full) {
                            pull = Pull::Full;
                        }
                    }
                    let since = match pull {
                        Pull::Full => 0,
                        Pull::Incremental => offset,
                    };
                    let (mut next, mut entries) = daemon::get_logs(since).await;
                    // An offset running backwards means the daemon restarted (its
                    // counter is per instance): start over from its first line.
                    let restart = matches!(pull, Pull::Incremental) && next < offset;
                    if restart {
                        (next, entries) = daemon::get_logs(0).await;
                    }
                    if restart || matches!(pull, Pull::Full) {
                        window.imp().text_view.buffer().set_text("");
                    }
                    window.append(&entries);
                    offset = next;
                }
            }
        ));

        let events = daemon::subscribe_events();
        let follower = glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                while let Ok(event) = events.recv().await {
                    match event {
                        DaemonEvent::LogsAppended(_) => window.request(Pull::Incremental),
                        DaemonEvent::DaemonRestarted => window.request(Pull::Full),
                        _ => {}
                    }
                }
            }
        ));
        imp.tasks.replace(vec![worker, follower]);
    }

    fn fill_filter(&self, library: &[LibraryItem]) {
        let mut ids = vec![None];
        let mut labels = vec![gettext("All Logs")];
        for item in library {
            match item {
                LibraryItem::Game(game) => {
                    ids.push(Some(game.id.clone()));
                    labels.push(game.title.clone());
                }
                LibraryItem::Group(group) => {
                    for game in &group.games {
                        ids.push(Some(game.id.clone()));
                        labels.push(format!("{}: {}", group.title, game.title));
                    }
                }
            }
        }
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let imp = self.imp();
        imp.filter_ids.replace(ids);
        imp.filter_dropdown
            .set_model(Some(&gtk4::StringList::new(&labels)));
    }

    fn select_game(&self, game_id: Option<&str>) {
        let imp = self.imp();
        let position = imp
            .filter_ids
            .borrow()
            .iter()
            .position(|id| id.as_deref() == game_id)
            .unwrap_or(0);
        imp.filter_dropdown.set_selected(position as u32);
        // Selecting the entry already selected notifies nothing.
        self.request(Pull::Full);
    }

    fn request(&self, pull: Pull) {
        if let Some(pulls) = self.imp().pulls.get() {
            let _ = pulls.try_send(pull);
        }
    }

    fn selected_game(&self) -> Option<String> {
        let imp = self.imp();
        imp.filter_ids
            .borrow()
            .get(imp.filter_dropdown.selected() as usize)
            .cloned()
            .flatten()
    }

    /// Appends the entries of the selected game in one insert, keeping the view
    /// bounded; the end stays in sight through [`Self::stick_to_end`].
    fn append(&self, entries: &[LogEntry]) {
        let imp = self.imp();
        let buffer = imp.text_view.buffer();
        let filter = self.selected_game();
        let mut text = String::new();
        for entry in entries
            .iter()
            .filter(|entry| filter.as_ref().is_none_or(|id| &entry.game_id == id))
        {
            // RFC3339 local timestamp → wall-clock "HH:MM:SS"; the date is noise
            // in a live log view.
            let time = entry.timestamp.get(11..19).unwrap_or(&entry.timestamp);
            let _ = writeln!(text, "[{time}] {}", entry.line);
        }
        if !text.is_empty() {
            buffer.insert(&mut buffer.end_iter(), &text);
            // The daemon retains a bounded ring; keep the view bounded too so a
            // chatty game cannot grow the buffer (and every later insert) forever.
            let excess = buffer.line_count() - MAX_VIEW_LINES;
            if excess > 0
                && let Some(mut cut) = buffer.iter_at_line(excess)
            {
                buffer.delete(&mut buffer.start_iter(), &mut cut);
            }
        }

        let empty = buffer.char_count() == 0;
        imp.stack
            .set_visible_child_name(if empty { "empty" } else { "log" });
        self.stick_to_end();
    }

    /// Scrolls to the end while following it. The text view does it once the new lines
    /// are laid out, gliding there rather than jumping.
    fn stick_to_end(&self) {
        let imp = self.imp();
        if imp.follow.get()
            && let Some(end) = imp.end_mark.get()
        {
            imp.text_view.scroll_to_mark(end, 0.0, true, 0.0, 1.0);
        }
    }
}
