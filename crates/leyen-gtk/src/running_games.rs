//! Running Games: what the daemon is tracking, how long each game has run and how
//! many processes it has, with its log and a stop button, and the programs run
//! in a prefix, which can be stopped too.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_ipc::{PrefixProgram, RunningGameSnapshot};
use leyen_model::i18n::{gettext, ngettext};
use leyen_model::icons::game_icon_path;
use leyen_model::library::flatten_games;
use libadwaita as adw;

use crate::daemon::{self, DaemonEvent};
use crate::format;
use crate::library_icon::LibraryIcon;
use crate::log_window::LogWindow;
use crate::playback;
use crate::window::LeyenWindow;

mod row_imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/running_game_row.ui")]
    pub struct RunningGameRow {
        #[template_child]
        pub icon: TemplateChild<LibraryIcon>,
        #[template_child]
        pub title_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub process_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub duration_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub logs_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub stop_button: TemplateChild<gtk4::Button>,
        pub started_at: std::cell::Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RunningGameRow {
        const NAME: &'static str = "LeyenRunningGameRow";
        type Type = super::RunningGameRow;
        type ParentType = gtk4::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            LibraryIcon::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for RunningGameRow {}
    impl WidgetImpl for RunningGameRow {}
    impl ListBoxRowImpl for RunningGameRow {}
}

glib::wrapper! {
    pub struct RunningGameRow(ObjectSubclass<row_imp::RunningGameRow>)
        @extends gtk4::ListBoxRow, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl RunningGameRow {
    fn update(&self, snapshot: &RunningGameSnapshot, title: &str) {
        let imp = self.imp();
        imp.title_label.set_label(title);
        self.update_property(&[gtk4::accessible::Property::Label(title)]);
        imp.icon.set_path(game_icon_path(&snapshot.game_id));
        let tracked = snapshot.tracked_pid_count as u32;
        let processes =
            ngettext("{} process", "{} processes", tracked).replacen("{}", &tracked.to_string(), 1);
        imp.process_label.set_label(
            &gettext("PID {} | tracking {}")
                .replacen("{}", &snapshot.pid.to_string(), 1)
                .replacen("{}", &processes, 1),
        );
        imp.logs_button
            .set_action_target_value(Some(&snapshot.game_id.to_variant()));
        imp.stop_button
            .set_action_target_value(Some(&snapshot.leyen_id.to_variant()));
        imp.started_at.set(snapshot.started_at_epoch_seconds);
        self.tick();
    }

    fn tick(&self) {
        self.imp()
            .duration_label
            .set_label(&format::running_for(self.imp().started_at.get()));
    }
}

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/running_games_window.ui")]
    pub struct RunningGamesWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub stack: TemplateChild<gtk4::Stack>,
        #[template_child]
        pub list: TemplateChild<gtk4::ListBox>,
        #[template_child]
        pub programs_box: TemplateChild<gtk4::Box>,
        #[template_child]
        pub programs_list: TemplateChild<gtk4::ListBox>,
        /// Rows by game id.
        pub rows: RefCell<HashMap<String, RunningGameRow>>,
        /// Program rows by scope unit.
        pub program_rows: RefCell<HashMap<String, adw::ActionRow>>,
        /// Game titles by game id, read again only when the library changes.
        pub titles: RefCell<HashMap<String, String>>,
        /// The running set shown now, redrawn with new titles.
        pub sessions: RefCell<Vec<RunningGameSnapshot>>,
        pub tasks: RefCell<Vec<glib::JoinHandle<()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RunningGamesWindow {
        const NAME: &'static str = "LeyenRunningGamesWindow";
        type Type = super::RunningGamesWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.install_action_async(
                "running.show-logs",
                Some(glib::VariantTy::STRING),
                |window, _, param| async move {
                    let game_id = param.and_then(|param| param.get::<String>());
                    if let Some(parent) = window.transient_for().and_downcast::<LeyenWindow>() {
                        LogWindow::present(&parent, game_id.as_deref()).await;
                    }
                },
            );
            klass.install_action_async(
                "running.stop",
                Some(glib::VariantTy::STRING),
                |window, _, param| async move {
                    let Some(leyen_id) = param.and_then(|param| param.get::<String>()) else {
                        return;
                    };
                    if let Some(message) = playback::stop(&leyen_id).await {
                        window
                            .imp()
                            .toast_overlay
                            .add_toast(adw::Toast::new(&message));
                    }
                },
            );
            klass.install_action_async(
                "running.stop-program",
                Some(glib::VariantTy::STRING),
                |window, _, param| async move {
                    let Some(unit) = param.and_then(|param| param.get::<String>()) else {
                        return;
                    };
                    if let Err(message) = daemon::stop_prefix_program(&unit).await {
                        window
                            .imp()
                            .toast_overlay
                            .add_toast(adw::Toast::new(&message));
                    }
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for RunningGamesWindow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }

        fn dispose(&self) {
            self.obj().stop_tasks();
        }
    }

    impl WidgetImpl for RunningGamesWindow {}

    impl WindowImpl for RunningGamesWindow {
        fn close_request(&self) -> glib::Propagation {
            self.obj().stop_tasks();
            self.parent_close_request()
        }
    }

    impl AdwWindowImpl for RunningGamesWindow {}
}

glib::wrapper! {
    pub struct RunningGamesWindow(ObjectSubclass<imp::RunningGamesWindow>)
        @extends adw::Window, gtk4::Window, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native,
                    gtk4::Root, gtk4::ShortcutManager;
}

impl RunningGamesWindow {
    /// Shows the window, the one there is when it is already open.
    pub fn present(parent: &LeyenWindow) {
        let existing = parent.application().and_then(|app| {
            app.windows()
                .into_iter()
                .find_map(|window| window.downcast::<Self>().ok())
        });
        let window = existing.unwrap_or_else(|| {
            glib::Object::builder()
                .property("application", parent.application())
                .property("transient-for", parent)
                .property("destroy-with-parent", true)
                .build()
        });
        window.present();
    }

    fn setup(&self) {
        self.imp().list.set_sort_func(|left, right| {
            let started = |row: &gtk4::ListBoxRow| {
                row.downcast_ref::<RunningGameRow>()
                    .map_or(0, |row| row.imp().started_at.get())
            };
            started(left).cmp(&started(right)).into()
        });

        let events = daemon::subscribe_events();
        let follower = glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                window.load_titles().await;
                window.show(daemon::running_games_snapshot().await);
                window.show_programs(&daemon::prefix_programs().await);
                // SessionsChanged carries the set and comes whenever a process count
                // moves, so it is drawn as it is, without asking the daemon again.
                while let Ok(event) = events.recv().await {
                    match event {
                        DaemonEvent::SessionsChanged(sessions) => window.show(sessions),
                        DaemonEvent::LibraryChanged => {
                            window.load_titles().await;
                            let sessions = window.imp().sessions.take();
                            window.show(sessions);
                        }
                        DaemonEvent::PrefixProgramsChanged(programs) => {
                            window.show_programs(&programs);
                        }
                        DaemonEvent::DaemonRestarted => {
                            window.load_titles().await;
                            window.show(daemon::running_games_snapshot().await);
                            window.show_programs(&daemon::prefix_programs().await);
                        }
                        _ => {}
                    }
                }
            }
        ));
        self.imp().tasks.replace(vec![follower]);

        // Retires itself once the window is gone or closed; removing the source by id
        // instead would fail on a tick that had already stopped.
        glib::timeout_add_seconds_local(
            1,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    if !window.is_visible() {
                        return glib::ControlFlow::Break;
                    }
                    for row in window.imp().rows.borrow().values() {
                        row.tick();
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );
    }

    fn stop_tasks(&self) {
        let imp = self.imp();
        for task in imp.tasks.take() {
            task.abort();
        }
    }

    async fn load_titles(&self) {
        let titles = flatten_games(&daemon::load_library().await.unwrap_or_default())
            .into_iter()
            .map(|game| (game.id, game.title))
            .collect();
        self.imp().titles.replace(titles);
    }

    fn show(&self, snapshots: Vec<RunningGameSnapshot>) {
        let imp = self.imp();
        let titles = imp.titles.borrow();
        let mut rows = imp.rows.borrow_mut();
        let mut stale: HashSet<String> = rows.keys().cloned().collect();
        for snapshot in &snapshots {
            stale.remove(&snapshot.game_id);
            let row = rows.entry(snapshot.game_id.clone()).or_insert_with(|| {
                let row: RunningGameRow = glib::Object::new();
                imp.list.append(&row);
                row
            });
            let title = titles
                .get(&snapshot.game_id)
                .map_or(snapshot.game_id.as_str(), String::as_str);
            row.update(snapshot, title);
        }
        for id in stale {
            if let Some(row) = rows.remove(&id) {
                imp.list.remove(&row);
            }
        }
        imp.list.invalidate_sort();
        imp.list.set_visible(!rows.is_empty());
        drop(rows);
        drop(titles);
        imp.sessions.replace(snapshots);
        self.update_stack();
    }

    fn show_programs(&self, programs: &[PrefixProgram]) {
        let imp = self.imp();
        let mut rows = imp.program_rows.borrow_mut();
        let mut stale: HashSet<String> = rows.keys().cloned().collect();
        for program in programs {
            stale.remove(&program.unit);
            let row = rows.entry(program.unit.clone()).or_insert_with(|| {
                let stop = gtk4::Button::builder()
                    .icon_name("media-playback-stop-symbolic")
                    .tooltip_text(gettext("Stop Program"))
                    .action_name("running.stop-program")
                    .action_target(&program.unit.to_variant())
                    .valign(gtk4::Align::Center)
                    .css_classes(["destructive-action", "circular"])
                    .build();
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .activatable(false)
                    .build();
                row.add_suffix(&stop);
                imp.programs_list.append(&row);
                row
            });
            row.set_title(&program.label);
            row.set_subtitle(&program.prefix);
        }
        for unit in stale {
            if let Some(row) = rows.remove(&unit) {
                imp.programs_list.remove(&row);
            }
        }
        imp.programs_box.set_visible(!rows.is_empty());
        drop(rows);
        self.update_stack();
    }

    fn update_stack(&self) {
        let imp = self.imp();
        let empty = imp.rows.borrow().is_empty() && imp.program_rows.borrow().is_empty();
        imp.stack
            .set_visible_child_name(if empty { "empty" } else { "list" });
    }
}
