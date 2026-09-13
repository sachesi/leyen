//! A component of the dependency manager: installed or not, and the buttons that
//! install, reinstall, remove or cancel it. The page runs the operations; the row
//! only shows where they are.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_model::deps::DepProfile;
use leyen_model::i18n::gettext;
use libadwaita as adw;

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/dependency_row.ui")]
    pub struct DependencyRow {
        #[template_child]
        pub badge: TemplateChild<gtk4::Label>,
        #[template_child]
        pub spinner: TemplateChild<adw::Spinner>,
        #[template_child]
        pub progress_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub cancel_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub install_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub reinstall_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub remove_button: TemplateChild<gtk4::Button>,
        pub profile: Cell<Option<&'static DepProfile>>,
        pub installed: Cell<bool>,
        /// Installed components that need this one.
        pub dependents: RefCell<Vec<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DependencyRow {
        const NAME: &'static str = "LeyenDependencyRow";
        type Type = super::DependencyRow;
        type ParentType = adw::ActionRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DependencyRow {}
    impl WidgetImpl for DependencyRow {}
    impl ListBoxRowImpl for DependencyRow {}
    impl PreferencesRowImpl for DependencyRow {}
    impl ActionRowImpl for DependencyRow {}
}

glib::wrapper! {
    pub struct DependencyRow(ObjectSubclass<imp::DependencyRow>)
        @extends adw::ActionRow, adw::PreferencesRow, gtk4::ListBoxRow, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl DependencyRow {
    pub fn new(profile: &'static DepProfile) -> Self {
        let row: Self = glib::Object::builder()
            .property("title", profile.name)
            .build();
        // Most catalogue entries describe themselves by their name alone.
        if profile.description != profile.name {
            row.set_subtitle(profile.description);
        }
        let imp = row.imp();
        imp.profile.set(Some(profile));
        let target = profile.id.to_variant();
        for button in [
            &imp.cancel_button,
            &imp.install_button,
            &imp.reinstall_button,
            &imp.remove_button,
        ] {
            button.set_action_target_value(Some(&target));
        }
        row
    }

    pub fn profile(&self) -> &'static DepProfile {
        self.imp().profile.get().expect("set in new()")
    }

    pub fn is_installed(&self) -> bool {
        self.imp().installed.get()
    }

    pub fn dependents(&self) -> Vec<String> {
        self.imp().dependents.borrow().clone()
    }

    /// Shows the component at rest. `dependents` are the installed components that
    /// need it, which keep it from being removed.
    pub fn set_idle(&self, installed: bool, integrated: bool, dependents: &[String]) {
        let imp = self.imp();
        imp.installed.set(installed);
        imp.dependents.replace(dependents.to_vec());
        imp.spinner.set_visible(false);
        imp.progress_label.set_visible(false);
        imp.cancel_button.set_visible(false);
        imp.badge.set_visible(installed);
        imp.badge.set_label(&if integrated {
            gettext("✓ Integrated")
        } else {
            gettext("✓ Installed")
        });
        imp.install_button.set_visible(!installed);
        imp.reinstall_button.set_visible(installed);
        imp.remove_button.set_visible(installed);
        imp.remove_button
            .set_tooltip_text(Some(&if dependents.is_empty() {
                gettext("Remove this managed dependency")
            } else {
                gettext("Required by: {}").replacen("{}", &dependents.join(", "), 1)
            }));
    }

    /// Shows an operation in progress; only installs can be cancelled.
    pub fn set_working(&self, cancellable: bool) {
        let imp = self.imp();
        imp.badge.set_visible(false);
        imp.install_button.set_visible(false);
        imp.reinstall_button.set_visible(false);
        imp.remove_button.set_visible(false);
        imp.spinner.set_visible(true);
        imp.progress_label.set_label("");
        imp.progress_label.set_visible(true);
        imp.cancel_button.set_sensitive(true);
        imp.cancel_button.set_visible(cancellable);
    }

    pub fn set_progress(&self, message: &str) {
        self.imp().progress_label.set_label(message);
    }

    /// Greys the cancel button out once a cancel is on its way.
    pub fn set_cancelling(&self) {
        self.imp().cancel_button.set_sensitive(false);
    }

    /// Whether a search for `query` (lowercase) finds the component.
    pub fn matches(&self, query: &str) -> bool {
        let profile = self.profile();
        query.is_empty()
            || profile.name.to_lowercase().contains(query)
            || profile.description.to_lowercase().contains(query)
            || profile.id.contains(query)
    }
}
