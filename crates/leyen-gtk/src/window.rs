//! The main window: the library, the page of an open group, and the `win.*` actions.
//!
//! Rows are kept across refreshes and updated in place, keyed by what they show, so
//! a refresh neither rebuilds the list nor moves it under the pointer. The daemon's
//! signals say when to refresh; nothing polls.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_ipc::RunningGameSnapshot;
use leyen_model::i18n::{gettext, ngettext};
use leyen_model::library::find_group;
use leyen_model::models::{Game, GameGroup, LibraryItem};
use libadwaita as adw;

use crate::daemon::{self, DaemonEvent};
use crate::desktop::{remove_game_desktop_entries, remove_game_desktop_entry};
use crate::dialogs::{GameDialog, GroupDialog};
use crate::game_row::GameRow;
use crate::group_row::GroupRow;
use crate::icons::{clear_game_icon, clear_group_icon};
use crate::log_window::LogWindow;
use crate::playback;
use crate::running_games::RunningGamesWindow;

/// Running games by the id of the game.
pub type RunningGameMap = HashMap<String, RunningGameSnapshot>;

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sachesi/leyen/ui/window.ui")]
    #[properties(wrapper_type = super::LeyenWindow)]
    pub struct LeyenWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub navigation_view: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub search_bar: TemplateChild<gtk4::SearchBar>,
        #[template_child]
        pub search_entry: TemplateChild<gtk4::SearchEntry>,
        #[template_child]
        pub library_stack: TemplateChild<gtk4::Stack>,
        #[template_child]
        pub library_list: TemplateChild<gtk4::ListBox>,
        #[template_child]
        pub group_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub group_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub group_edit_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub group_stack: TemplateChild<gtk4::Stack>,
        #[template_child]
        pub group_list: TemplateChild<gtk4::ListBox>,
        /// Set while umu-launcher or winetricks is still being downloaded.
        #[property(get, set)]
        pub runtime_busy: Cell<bool>,
        #[property(get, set)]
        pub runtime_message: RefCell<String>,
        pub library: RefCell<Vec<LibraryItem>>,
        /// Rows of the library, by "game:<id>" and "group:<id>".
        pub library_rows: RefCell<HashMap<String, gtk4::ListBoxRow>>,
        /// Rows of the open group's page, by game id.
        pub group_rows: RefCell<HashMap<String, GameRow>>,
        pub open_group: RefCell<Option<String>>,
        /// A refresh is running; the ones asked for meanwhile collapse into one more.
        pub refresh_busy: Cell<bool>,
        pub refresh_pending: Cell<bool>,
        /// The library changed while the window was hidden.
        pub stale_while_hidden: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LeyenWindow {
        const NAME: &'static str = "LeyenWindow";
        type Type = super::LeyenWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            super::install_actions(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for LeyenWindow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for LeyenWindow {}

    impl WindowImpl for LeyenWindow {
        /// While a game runs the window only hides, and closes once the last game ends.
        fn close_request(&self) -> glib::Propagation {
            if daemon::is_any_game_running() {
                self.obj().set_visible(false);
                return glib::Propagation::Stop;
            }
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for LeyenWindow {}
    impl AdwApplicationWindowImpl for LeyenWindow {}

    #[gtk4::template_callbacks]
    impl LeyenWindow {
        #[template_callback]
        fn on_search_changed(&self, _entry: &gtk4::SearchEntry) {
            self.library_list.invalidate_filter();
            self.obj().update_library_stack();
        }

        #[template_callback]
        fn on_row_activated(&self, row: &gtk4::ListBoxRow, _list: &gtk4::ListBox) {
            let obj = self.obj();
            if let Some(row) = row.downcast_ref::<GameRow>() {
                let _ = WidgetExt::activate_action(
                    &*obj,
                    "win.play-game",
                    Some(&row.game_id().to_variant()),
                );
            } else if let Some(row) = row.downcast_ref::<GroupRow>() {
                obj.open_group(&row.group_id());
            }
        }

        #[template_callback]
        fn on_page_popped(&self, page: &adw::NavigationPage, _view: &adw::NavigationView) {
            if page == &*self.group_page {
                self.open_group.replace(None);
                for (_, row) in self.group_rows.take() {
                    self.group_list.remove(&row);
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct LeyenWindow(ObjectSubclass<imp::LeyenWindow>)
        @extends adw::ApplicationWindow, gtk4::ApplicationWindow, gtk4::Window, gtk4::Widget,
        @implements gtk4::gio::ActionGroup, gtk4::gio::ActionMap, gtk4::Accessible,
                    gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native, gtk4::Root,
                    gtk4::ShortcutManager;
}

fn string_param(param: Option<&glib::Variant>) -> Option<String> {
    param.and_then(|param| param.get::<String>())
}

fn install_actions(klass: &mut <imp::LeyenWindow as ObjectSubclass>::Class) {
    klass.install_action_async("win.add-game", None, |win, _, _| async move {
        let group = win
            .group_page_visible()
            .then(|| win.imp().open_group.borrow().clone())
            .flatten();
        GameDialog::present_add(&win, group).await;
    });
    klass.install_action_async("win.add-group", None, |win, _, _| async move {
        GroupDialog::present_add(&win).await;
    });
    klass.install_action_async(
        "win.edit-game",
        Some(glib::VariantTy::STRING),
        |win, _, param| async move {
            if let Some((game, group)) =
                string_param(param.as_ref()).and_then(|id| win.find_game(&id))
            {
                GameDialog::present_edit(&win, game, group).await;
            }
        },
    );
    klass.install_action_async(
        "win.edit-group",
        Some(glib::VariantTy::STRING),
        |win, _, param| async move {
            let group = string_param(param.as_ref())
                .and_then(|id| find_group(&win.imp().library.borrow(), &id).cloned());
            if let Some(group) = group {
                GroupDialog::present_edit(&win, group).await;
            }
        },
    );
    klass.install_action_async(
        "win.delete-item",
        Some(glib::VariantTy::STRING),
        |win, _, param| async move {
            if let Some(id) = string_param(param.as_ref()) {
                win.confirm_delete(&id).await;
            }
        },
    );
    klass.install_action_async(
        "win.play-game",
        Some(glib::VariantTy::STRING),
        |win, _, param| async move {
            let Some((game, _)) = string_param(param.as_ref()).and_then(|id| win.find_game(&id))
            else {
                return;
            };
            if let Some(message) = playback::launch_or_stop(&game).await {
                win.toast(&message);
            }
        },
    );
    klass.install_action(
        "win.open-group",
        Some(glib::VariantTy::STRING),
        |win, _, param| {
            if let Some(id) = string_param(param) {
                win.open_group(&id);
            }
        },
    );
    klass.install_action_async("win.show-logs", None, |win, _, _| async move {
        LogWindow::present(&win, None).await;
    });
    klass.install_action("win.show-running-games", None, |win, _, _| {
        RunningGamesWindow::present(win);
    });
    klass.install_action("win.toggle-search", None, |win, _, _| {
        let imp = win.imp();
        if win.group_page_visible() {
            imp.navigation_view.pop();
        }
        let enable = !imp.search_bar.is_search_mode();
        imp.search_bar.set_search_mode(enable);
        if enable {
            imp.search_entry.grab_focus();
        }
    });
}

/// Running games first; then groups, then games, each by title.
fn library_order(left: &gtk4::ListBoxRow, right: &gtk4::ListBoxRow) -> Ordering {
    let running_game = |row: &gtk4::ListBoxRow| {
        row.downcast_ref::<GameRow>()
            .is_some_and(GameRow::is_running)
    };
    running_game(right).cmp(&running_game(left)).then_with(|| {
        match (
            left.downcast_ref::<GroupRow>(),
            right.downcast_ref::<GroupRow>(),
        ) {
            (Some(left), Some(right)) => title_order(&left.title(), &right.title()),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => game_order(left, right),
        }
    })
}

/// Running games first, then by title.
fn game_order(left: &gtk4::ListBoxRow, right: &gtk4::ListBoxRow) -> Ordering {
    match (
        left.downcast_ref::<GameRow>(),
        right.downcast_ref::<GameRow>(),
    ) {
        (Some(left), Some(right)) => right
            .is_running()
            .cmp(&left.is_running())
            .then_with(|| title_order(&left.title(), &right.title())),
        _ => Ordering::Equal,
    }
}

fn title_order(left: &str, right: &str) -> Ordering {
    left.to_lowercase().cmp(&right.to_lowercase())
}

impl LeyenWindow {
    pub fn new(app: &impl IsA<adw::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    pub fn toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    /// Reloads the library and the running games, and shows them.
    pub fn refresh(&self) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = win)]
            self,
            async move { win.refresh_future().await }
        ));
    }

    pub async fn refresh_future(&self) {
        let imp = self.imp();
        if imp.refresh_busy.replace(true) {
            imp.refresh_pending.set(true);
            return;
        }
        // Cleared on drop, so a panic in a refresh cannot wedge every later one.
        struct Busy<'a>(&'a Cell<bool>);
        impl Drop for Busy<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _busy = Busy(&imp.refresh_busy);
        loop {
            imp.refresh_pending.set(false);
            self.reload().await;
            if !imp.refresh_pending.get() {
                break;
            }
        }
    }

    async fn reload(&self) {
        let (items, running) =
            futures_util::join!(daemon::load_library(), daemon::running_games_snapshot());
        let items = match items {
            Ok(items) => items,
            Err(err) => {
                self.toast(&err);
                return;
            }
        };
        let running: RunningGameMap = running
            .into_iter()
            .map(|snapshot| (snapshot.game_id.clone(), snapshot))
            .collect();
        if !self.is_visible() {
            self.imp().stale_while_hidden.set(true);
            return;
        }
        self.apply(items, &running);
    }

    fn setup(&self) {
        let imp = self.imp();
        imp.search_bar.connect_entry(&*imp.search_entry);
        imp.library_list
            .set_sort_func(|left, right| library_order(left, right).into());
        imp.library_list.set_filter_func(glib::clone!(
            #[weak(rename_to = win)]
            self,
            #[upgrade_or]
            false,
            move |row| win.library_row_visible(row)
        ));
        imp.group_list
            .set_sort_func(|left, right| game_order(left, right).into());

        // Typing on a group's page opens the search of the library, which lives there.
        imp.search_bar
            .connect_search_mode_enabled_notify(glib::clone!(
                #[weak(rename_to = win)]
                self,
                move |bar| {
                    if bar.is_search_mode() && win.group_page_visible() {
                        win.imp().navigation_view.pop();
                    }
                }
            ));

        self.connect_map(|win| {
            if win.imp().stale_while_hidden.replace(false) {
                win.refresh();
            }
        });

        // Advances the running times while a game runs; the rows know when each started.
        glib::timeout_add_seconds_local(
            1,
            glib::clone!(
                #[weak(rename_to = win)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    if win.is_visible() && daemon::is_any_game_running() {
                        win.tick();
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );

        self.watch_daemon();
        self.pull_runtime_status();
        self.refresh();
    }

    fn watch_daemon(&self) {
        let events = daemon::subscribe_events();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = win)]
            self,
            async move {
                // SessionsChanged comes on every monitor tick, not only when the
                // set of running games changes; refresh only when it does.
                let mut last_sessions = None;
                while let Ok(event) = events.recv().await {
                    match event {
                        DaemonEvent::SessionsChanged(sessions) => {
                            let identity = daemon::sessions_identity(&sessions);
                            let changed = last_sessions.as_ref() != Some(&identity);
                            last_sessions = Some(identity);
                            if !win.is_visible() {
                                // Hidden while a game ran: close with the last one.
                                if sessions.is_empty() {
                                    win.close();
                                } else if changed {
                                    win.imp().stale_while_hidden.set(true);
                                }
                            } else if changed {
                                win.refresh_future().await;
                            }
                        }
                        DaemonEvent::LibraryChanged | DaemonEvent::DaemonRestarted => {
                            if matches!(event, DaemonEvent::DaemonRestarted) {
                                // A fresh daemon has lost the readiness it cached.
                                win.pull_runtime_status();
                            }
                            if win.is_visible() {
                                win.refresh_future().await;
                            } else {
                                win.imp().stale_while_hidden.set(true);
                            }
                        }
                        DaemonEvent::RuntimeStatus {
                            umu_ready,
                            winetricks_ready,
                        } => win.show_runtime_status(umu_ready, winetricks_ready),
                        DaemonEvent::Error(message) => win.toast(&message),
                        _ => {}
                    }
                }
            }
        ));
    }

    fn pull_runtime_status(&self) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = win)]
            self,
            async move {
                if let Some(status) = daemon::get_runtime_status().await {
                    win.show_runtime_status(status.umu_ready, status.winetricks_ready);
                }
            }
        ));
    }

    fn show_runtime_status(&self, umu_ready: bool, winetricks_ready: bool) {
        let message = match (umu_ready, winetricks_ready) {
            (true, true) => None,
            (false, false) => Some(gettext(
                "Downloading umu-launcher & winetricks… Please wait before starting games.",
            )),
            (false, true) => Some(gettext(
                "Downloading umu-launcher… Please wait before starting games.",
            )),
            (true, false) => Some(gettext("Downloading winetricks…")),
        };
        if let Some(message) = &message {
            self.set_runtime_message(message.as_str());
        }
        self.set_runtime_busy(message.is_some());
    }

    fn apply(&self, items: Vec<LibraryItem>, running: &RunningGameMap) {
        let imp = self.imp();
        let list = &imp.library_list;
        {
            let mut rows = imp.library_rows.borrow_mut();
            let mut stale: HashSet<String> = rows.keys().cloned().collect();
            let mut game_row = |game: &Game, group: Option<(&str, &str)>| {
                let key = format!("game:{}", game.id);
                stale.remove(&key);
                let row = rows.entry(key).or_insert_with(|| {
                    let row = GameRow::default();
                    list.append(&row);
                    row.upcast()
                });
                if let Some(row) = row.downcast_ref::<GameRow>() {
                    row.update(game, group, running_since(running, &game.id));
                }
            };
            for item in &items {
                match item {
                    LibraryItem::Game(game) => game_row(game, None),
                    LibraryItem::Group(group) => {
                        for game in &group.games {
                            game_row(game, Some((&group.id, &group.title)));
                        }
                    }
                }
            }
            for item in &items {
                if let LibraryItem::Group(group) = item {
                    let key = format!("group:{}", group.id);
                    stale.remove(&key);
                    let row = rows.entry(key).or_insert_with(|| {
                        let row = GroupRow::default();
                        list.append(&row);
                        row.upcast()
                    });
                    if let Some(row) = row.downcast_ref::<GroupRow>() {
                        row.update(group, running);
                    }
                }
            }
            for key in stale {
                if let Some(row) = rows.remove(&key) {
                    list.remove(&row);
                }
            }
        }
        list.invalidate_sort();
        list.invalidate_filter();
        imp.library.replace(items);
        self.update_library_stack();
        self.sync_group_page(running);
    }

    fn sync_group_page(&self, running: &RunningGameMap) {
        let imp = self.imp();
        let Some(group_id) = imp.open_group.borrow().clone() else {
            return;
        };
        let group = find_group(&imp.library.borrow(), &group_id).cloned();
        let Some(group) = group else {
            // Deleted elsewhere while it was open.
            imp.navigation_view.pop_to_tag("library");
            return;
        };
        imp.group_page.set_title(&group.title);
        imp.group_title.set_title(&group.title);
        imp.group_edit_button
            .set_action_target_value(Some(&group.id.to_variant()));

        let mut rows = imp.group_rows.borrow_mut();
        let mut stale: HashSet<String> = rows.keys().cloned().collect();
        for game in &group.games {
            stale.remove(&game.id);
            let row = rows.entry(game.id.clone()).or_insert_with(|| {
                let row = GameRow::default();
                imp.group_list.append(&row);
                row
            });
            row.update(
                game,
                Some((&group.id, &group.title)),
                running_since(running, &game.id),
            );
        }
        for id in stale {
            if let Some(row) = rows.remove(&id) {
                imp.group_list.remove(&row);
            }
        }
        imp.group_list.invalidate_sort();
        imp.group_stack
            .set_visible_child_name(if rows.is_empty() { "empty" } else { "list" });
    }

    fn library_row_visible(&self, row: &gtk4::ListBoxRow) -> bool {
        let query = self.imp().search_entry.text().trim().to_lowercase();
        match row.downcast_ref::<GameRow>() {
            // Grouped games are listed by themselves only in search results.
            Some(game) if query.is_empty() => !game.in_group(),
            Some(game) => game.matches(&query),
            None => query.is_empty(),
        }
    }

    fn update_library_stack(&self) {
        let imp = self.imp();
        let rows = imp.library_rows.borrow();
        let page = if rows.is_empty() {
            "empty"
        } else if rows.values().any(|row| self.library_row_visible(row)) {
            "list"
        } else {
            "no-results"
        };
        imp.library_stack.set_visible_child_name(page);
    }

    fn tick(&self) {
        let imp = self.imp();
        for row in imp.library_rows.borrow().values() {
            if let Some(row) = row.downcast_ref::<GameRow>() {
                row.tick();
            } else if let Some(row) = row.downcast_ref::<GroupRow>() {
                row.tick();
            }
        }
        for row in imp.group_rows.borrow().values() {
            row.tick();
        }
    }

    fn group_page_visible(&self) -> bool {
        let imp = self.imp();
        imp.navigation_view.visible_page().as_ref() == Some(imp.group_page.upcast_ref())
    }

    pub fn open_group(&self, group_id: &str) {
        let imp = self.imp();
        imp.open_group.replace(Some(group_id.to_string()));
        let running = daemon::cached_running_games()
            .into_iter()
            .map(|snapshot| (snapshot.game_id.clone(), snapshot))
            .collect();
        self.sync_group_page(&running);
        if !self.group_page_visible() {
            imp.navigation_view.push_by_tag("group");
        }
    }

    /// The game with this id and the group it is in, as last loaded.
    fn find_game(&self, game_id: &str) -> Option<(Game, Option<GameGroup>)> {
        self.imp()
            .library
            .borrow()
            .iter()
            .find_map(|item| match item {
                LibraryItem::Game(game) if game.id == game_id => Some((game.clone(), None)),
                LibraryItem::Group(group) => group
                    .games
                    .iter()
                    .find(|game| game.id == game_id)
                    .map(|game| (game.clone(), Some(group.clone()))),
                LibraryItem::Game(_) => None,
            })
    }

    /// Refuses to delete a running game, or a group with one, with a dialog saying so:
    /// once gone from the library a game could no longer be stopped.
    async fn refuse_delete_if_running(&self, item_id: &str) -> bool {
        let running = daemon::running_games_snapshot().await;
        let is_running = |id: &str| running.iter().any(|snapshot| snapshot.game_id == id);
        let (heading, body) = if let Some((game, _)) = self.find_game(item_id) {
            if !is_running(&game.id) {
                return false;
            }
            (
                // Translators: the title of the game.
                gettext("“{}” Is Running").replacen("{}", &game.title, 1),
                gettext("Stop the game before deleting it."),
            )
        } else if let Some(group) = find_group(&self.imp().library.borrow(), item_id) {
            let count = group
                .games
                .iter()
                .filter(|game| is_running(&game.id))
                .count() as u32;
            if count == 0 {
                return false;
            }
            (
                // Translators: the title of the group.
                ngettext(
                    "A Game in “{}” Is Running",
                    "Games in “{}” Are Running",
                    count,
                )
                .replacen("{}", &group.title, 1),
                ngettext(
                    "Stop it before deleting the group.",
                    "Stop them before deleting the group.",
                    count,
                ),
            )
        } else {
            return false;
        };
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
        dialog.add_response("close", &gettext("OK"));
        dialog.choose_future(Some(self)).await;
        true
    }

    async fn confirm_delete(&self, item_id: &str) {
        if self.refuse_delete_if_running(item_id).await {
            return;
        }
        let (title, body) = match (
            self.find_game(item_id),
            find_group(&self.imp().library.borrow(), item_id),
        ) {
            (Some((game, _)), _) => (game.title, gettext("Its playtime goes with it.")),
            (None, Some(group)) if !group.games.is_empty() => {
                let count = group.games.len() as u32;
                (
                    group.title.clone(),
                    ngettext(
                        "Its {} game and their playtime go with it.",
                        "Its {} games and their playtime go with it.",
                        count,
                    )
                    .replacen("{}", &count.to_string(), 1),
                )
            }
            (None, Some(group)) => (group.title.clone(), String::new()),
            (None, None) => return,
        };
        let body = [body, gettext("This cannot be undone.")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        // Translators: the title of the game or group about to be deleted.
        let heading = gettext("Delete “{}”?").replacen("{}", &title, 1);
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
        dialog.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("delete", &gettext("Delete")),
        ]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        // Checked again: the game can be started from its menu entry meanwhile.
        if dialog.choose_future(Some(self)).await != "delete"
            || self.refuse_delete_if_running(item_id).await
        {
            return;
        }

        let mut items = match daemon::load_library().await {
            Ok(items) => items,
            Err(err) => {
                self.toast(&err);
                return;
            }
        };
        let game = leyen_model::library::remove_game(&mut items, item_id);
        let group = match game {
            Some(_) => None,
            None => leyen_model::library::remove_group(&mut items, item_id),
        };
        if game.is_none() && group.is_none() {
            return;
        }
        if let Err(reason) = daemon::save_library(items).await {
            self.toast(&reason);
            self.refresh_future().await;
            return;
        }

        // After the save: a refused save leaves the item, so its icon and menu entry
        // must stay too.
        let mut notice = None;
        let title = match (game, group) {
            (Some(game), _) => {
                let id = game.id.clone();
                let _ = daemon::gio_blocking(move || clear_game_icon(&id)).await;
                if let Err(err) = remove_game_desktop_entry(game.leyen_id.clone()).await {
                    notice =
                        Some(gettext("Failed to remove menu entry: {}").replacen("{}", &err, 1));
                }
                game.title
            }
            (None, Some(group)) => {
                let group_id = group.id.clone();
                let game_ids: Vec<String> = group.games.iter().map(|g| g.id.clone()).collect();
                let _ = daemon::gio_blocking(move || {
                    clear_group_icon(&group_id);
                    for id in &game_ids {
                        clear_game_icon(id);
                    }
                })
                .await;
                let leyen_ids = group.games.iter().map(|g| g.leyen_id.clone()).collect();
                if let Err(err) = remove_game_desktop_entries(leyen_ids).await {
                    notice =
                        Some(gettext("Failed to remove a menu entry: {}").replacen("{}", &err, 1));
                }
                group.title
            }
            (None, None) => return,
        };
        self.refresh_future().await;
        let message = match notice {
            Some(notice) => gettext("'{}' deleted successfully. {}")
                .replacen("{}", &title, 1)
                .replacen("{}", &notice, 1),
            None => gettext("'{}' deleted successfully").replacen("{}", &title, 1),
        };
        self.toast(&message);
    }
}

fn running_since(running: &RunningGameMap, game_id: &str) -> Option<u64> {
    running
        .get(game_id)
        .map(|snapshot| snapshot.started_at_epoch_seconds)
}
