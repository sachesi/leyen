//! The dependency manager of a prefix: winetricks components by category, the
//! installed ones first. A page pushed over the dialog that opened it, never a
//! second dialog. One operation runs at a time; leaving the page does not stop it.

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use futures_util::future::{Either, select};
use gtk4::glib;
use leyen_model::deps::{
    DEP_CATEGORY_ORDER, DEP_PROFILES, find_installed_dependents, get_dep_profile,
    get_installed_dep, read_prefix_dep_state,
};
use leyen_model::i18n::{gettext, ngettext};
use leyen_model::paths::get_data_dir;
use libadwaita as adw;

use super::dependency_row::DependencyRow;
use crate::daemon::{self, DaemonEvent, gio_blocking};

/// Backstop for a job whose daemon dies without ever sending `DepFinished`,
/// `DaemonRestarted`, or `Error`: without this the row would spin forever.
const DEP_JOB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Install,
    Reinstall,
    Remove,
}

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/dependencies_page.ui")]
    pub struct DependenciesPage {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub search_bar: TemplateChild<gtk4::SearchBar>,
        #[template_child]
        pub search_entry: TemplateChild<gtk4::SearchEntry>,
        #[template_child]
        pub page: TemplateChild<adw::PreferencesPage>,
        pub prefix: RefCell<String>,
        pub proton: RefCell<String>,
        pub rows: RefCell<Vec<DependencyRow>>,
        pub groups: RefCell<Vec<adw::PreferencesGroup>>,
        /// The daemon's id of the job running now, for Cancel.
        pub job: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DependenciesPage {
        const NAME: &'static str = "LeyenDependenciesPage";
        type Type = super::DependenciesPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            for (name, operation) in [
                ("deps.install", Operation::Install),
                ("deps.reinstall", Operation::Reinstall),
                ("deps.remove", Operation::Remove),
            ] {
                klass.install_action_async(
                    name,
                    Some(glib::VariantTy::STRING),
                    move |page, _, param| async move {
                        if let Some(id) = param.and_then(|param| param.get::<String>()) {
                            page.run_operation(operation, &id).await;
                        }
                    },
                );
            }
            klass.install_action(
                "deps.cancel",
                Some(glib::VariantTy::STRING),
                |page, _, _| {
                    page.cancel();
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DependenciesPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.search_bar.connect_entry(&*self.search_entry);
        }
    }

    impl WidgetImpl for DependenciesPage {}
    impl NavigationPageImpl for DependenciesPage {}

    #[gtk4::template_callbacks]
    impl DependenciesPage {
        /// Typing searches straight away: the row that opened the page kept the focus.
        #[template_callback]
        fn on_shown(&self, _page: &adw::NavigationPage) {
            self.search_entry.grab_focus();
        }

        #[template_callback]
        fn on_search_changed(&self, _entry: &gtk4::SearchEntry) {
            self.obj().apply_filter();
        }
    }
}

glib::wrapper! {
    pub struct DependenciesPage(ObjectSubclass<imp::DependenciesPage>)
        @extends adw::NavigationPage, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

fn category_position(category: &str) -> usize {
    DEP_CATEGORY_ORDER
        .iter()
        .position(|candidate| *candidate == category)
        .unwrap_or(usize::MAX)
}

impl DependenciesPage {
    /// The page for `prefix`, or why it cannot be opened now. An empty prefix means
    /// the default one.
    pub async fn open(prefix: &str, proton: &str) -> Result<Self, String> {
        if !daemon::running_games_snapshot().await.is_empty() {
            return Err(gettext(
                "Dependency manager is blocked while games are running. Close all games first.",
            ));
        }
        let prefix = if prefix.trim().is_empty() {
            let default = daemon::load_settings().await.default_prefix_path;
            if default.is_empty() {
                get_data_dir()
                    .join("prefixes")
                    .join("default")
                    .to_string_lossy()
                    .into_owned()
            } else {
                default
            }
        } else {
            prefix.to_string()
        };

        let page: Self = glib::Object::new();
        let imp = page.imp();
        imp.prefix.replace(prefix);
        imp.proton.replace(proton.to_string());

        let mut profiles: Vec<_> = DEP_PROFILES.iter().collect();
        profiles.sort_by(|left, right| {
            category_position(left.category)
                .cmp(&category_position(right.category))
                .then(left.name.cmp(right.name))
        });
        imp.rows
            .replace(profiles.into_iter().map(DependencyRow::new).collect());
        page.refresh().await;
        Ok(page)
    }

    /// Reads what the prefix has installed and shows it.
    async fn refresh(&self) {
        let imp = self.imp();
        let prefix = imp.prefix.borrow().clone();
        let state = gio_blocking(move || read_prefix_dep_state(&prefix))
            .await
            .unwrap_or_default();
        let installed = state.installed.len();
        imp.window_title.set_subtitle(&if installed == 0 {
            gettext("No components installed")
        } else {
            ngettext(
                "{} component installed",
                "{} components installed",
                installed as u32,
            )
            .replacen("{}", &installed.to_string(), 1)
        });
        for row in imp.rows.borrow().iter() {
            let id = row.profile().id;
            let dependents: Vec<String> = find_installed_dependents(&state, id)
                .into_iter()
                .map(|dependent| {
                    get_dep_profile(&dependent)
                        .map(|profile| profile.name.to_string())
                        .unwrap_or(dependent)
                })
                .collect();
            let info = state.installed.get(id);
            row.set_idle(
                info.is_some(),
                info.is_some_and(|info| info.is_prefix_integration()),
                &dependents,
            );
        }
        self.arrange();
    }

    /// Puts every row in its group: Installed first, then the categories.
    fn arrange(&self) {
        let imp = self.imp();
        for group in imp.groups.take() {
            for row in imp.rows.borrow().iter() {
                if row.is_ancestor(&group) {
                    group.remove(row);
                }
            }
            imp.page.remove(&group);
        }

        let rows = imp.rows.borrow();
        let mut groups: Vec<(String, adw::PreferencesGroup)> = Vec::new();
        if rows.iter().any(DependencyRow::is_installed) {
            groups.push(("".into(), group_titled(&gettext("Installed"))));
        }
        for row in rows.iter() {
            let key = if row.is_installed() {
                ""
            } else {
                row.profile().category
            };
            let position = match groups.iter().position(|(category, _)| category == key) {
                Some(position) => position,
                None => {
                    groups.push((key.to_string(), group_titled(key)));
                    groups.len() - 1
                }
            };
            groups[position].1.add(row);
        }
        for (_, group) in &groups {
            imp.page.add(group);
        }
        imp.groups
            .replace(groups.into_iter().map(|(_, group)| group).collect());
        drop(rows);
        self.apply_filter();
    }

    fn apply_filter(&self) {
        let imp = self.imp();
        let query = imp.search_entry.text().trim().to_lowercase();
        for row in imp.rows.borrow().iter() {
            row.set_visible(row.matches(&query));
        }
        // By the query, not by visibility: a row in a hidden group is never visible.
        for group in imp.groups.borrow().iter() {
            let any_match = imp
                .rows
                .borrow()
                .iter()
                .any(|row| row.is_ancestor(group) && row.matches(&query));
            group.set_visible(any_match);
        }
    }

    fn set_busy(&self, busy: bool) {
        for action in ["deps.install", "deps.reinstall", "deps.remove"] {
            self.action_set_enabled(action, !busy);
        }
        self.imp().search_entry.set_sensitive(!busy);
    }

    fn toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn row(&self, dep_id: &str) -> Option<DependencyRow> {
        self.imp()
            .rows
            .borrow()
            .iter()
            .find(|row| row.profile().id == dep_id)
            .cloned()
    }

    async fn run_operation(&self, operation: Operation, dep_id: &str) {
        let Some(row) = self.row(dep_id) else {
            return;
        };
        let profile = row.profile();
        if operation == Operation::Remove {
            let dependents = row.dependents();
            if !dependents.is_empty() {
                self.toast(&gettext("Required by: {}").replacen("{}", &dependents.join(", "), 1));
                return;
            }
            if !self.confirm_remove(profile.id, profile.name).await {
                return;
            }
        }

        self.set_busy(true);
        row.set_working(operation != Operation::Remove);
        let (success, note) = self.run_job(operation, profile.id, &row).await;
        self.imp().job.replace(None);
        self.refresh().await;
        self.set_busy(false);

        let name = profile.name;
        let message = match (operation, success, note) {
            (Operation::Install, true, Some(note)) => gettext("'{}' installed successfully. {}")
                .replacen("{}", name, 1)
                .replacen("{}", &note, 1),
            (Operation::Install, true, None) => {
                gettext("'{}' installed successfully.").replacen("{}", name, 1)
            }
            (Operation::Reinstall, true, Some(note)) => {
                gettext("'{}' reinstalled successfully. {}")
                    .replacen("{}", name, 1)
                    .replacen("{}", &note, 1)
            }
            (Operation::Reinstall, true, None) => {
                gettext("'{}' reinstalled successfully.").replacen("{}", name, 1)
            }
            (Operation::Remove, true, Some(note)) => gettext("'{}' removed successfully. {}")
                .replacen("{}", name, 1)
                .replacen("{}", &note, 1),
            (Operation::Remove, true, None) => {
                gettext("'{}' removed successfully.").replacen("{}", name, 1)
            }
            (_, false, Some(reason)) => reason,
            (Operation::Install, false, None) => gettext("Installation failed."),
            (Operation::Reinstall, false, None) => gettext("Reinstall failed."),
            (Operation::Remove, false, None) => gettext("Remove failed."),
        };
        self.toast(&message);
    }

    async fn confirm_remove(&self, dep_id: &'static str, name: &str) -> bool {
        let prefix = self.imp().prefix.borrow().clone();
        let detail = gio_blocking(move || {
            get_installed_dep(&prefix, dep_id).map(|installed| installed.removal_detail())
        })
        .await
        .flatten()
        .unwrap_or_else(|| gettext("This removes the dependency from Leyen's tracking."));

        let dialog = adw::AlertDialog::new(
            Some(&gettext("Remove '{}'?").replacen("{}", name, 1)),
            Some(&detail),
        );
        dialog.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("remove", &gettext("Remove")),
        ]);
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.choose_future(Some(self)).await == "remove"
    }

    /// Runs the job in the daemon, following its progress on the row. Returns
    /// whether it succeeded and the daemon's note or reason.
    async fn run_job(
        &self,
        operation: Operation,
        dep_id: &str,
        row: &DependencyRow,
    ) -> (bool, Option<String>) {
        let imp = self.imp();
        let prefix = imp.prefix.borrow().clone();
        let proton = imp.proton.borrow().clone();
        // Subscribe before starting so no early progress is missed.
        let events = daemon::subscribe_events();
        let started = if operation == Operation::Remove {
            daemon::uninstall_dep(&prefix, dep_id, &proton).await
        } else {
            daemon::install_dep(&prefix, dep_id, &proton).await
        };
        let job_id = match started {
            Ok(job_id) => job_id,
            Err(reason) => return (false, Some(reason)),
        };
        imp.job.replace(Some(job_id.clone()));

        let deadline = std::time::Instant::now() + DEP_JOB_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return (false, Some(gettext("Timed out waiting for the daemon.")));
            }
            let event = match select(
                Box::pin(events.recv()),
                Box::pin(glib::timeout_future(remaining)),
            )
            .await
            {
                Either::Left((Ok(event), _)) => event,
                Either::Left((Err(_), _)) => {
                    return (false, Some(gettext("Timed out waiting for the daemon.")));
                }
                Either::Right(_) => continue,
            };
            match event {
                DaemonEvent::DepProgress {
                    job_id: job, msg, ..
                } if job == job_id => {
                    row.set_progress(&msg);
                }
                DaemonEvent::DepFinished {
                    job_id: job,
                    success,
                    message,
                } if job == job_id => {
                    return (success, (!message.is_empty()).then_some(message));
                }
                DaemonEvent::DaemonRestarted => {
                    return (
                        false,
                        Some(gettext(
                            "The daemon restarted; the operation's outcome is unknown.",
                        )),
                    );
                }
                DaemonEvent::Error(message) => return (false, Some(message)),
                _ => {}
            }
        }
    }

    fn cancel(&self) {
        let Some(job) = self.imp().job.borrow().clone() else {
            return;
        };
        for row in self.imp().rows.borrow().iter() {
            row.set_cancelling();
        }
        glib::spawn_future_local(async move {
            daemon::cancel_dep(&job).await;
        });
    }
}

fn group_titled(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}
