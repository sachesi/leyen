//! The Tools group of the preferences and of the game and group dialogs: the Wine
//! configuration, the registry editor, the dependency manager and running a program,
//! each in the prefix the dialog is about. When the prefix is managed elsewhere the
//! group says where instead.

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_model::i18n::gettext;
use libadwaita as adw;

use super::DependenciesPage;
use crate::prefix_tools;

/// The prefix the tools act on and the Proton that runs them ("" for umu's own).
pub struct ToolTarget {
    pub prefix: String,
    pub proton: String,
}

type TargetFn = Box<dyn Fn() -> Result<ToolTarget, String>>;

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/prefix_tools_group.ui")]
    pub struct PrefixToolsGroup {
        #[template_child]
        pub notice_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub notice_icon: TemplateChild<gtk4::Image>,
        #[template_child]
        pub winecfg_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub regedit_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub dependencies_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub run_row: TemplateChild<adw::ActionRow>,
        pub target: RefCell<Option<TargetFn>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PrefixToolsGroup {
        const NAME: &'static str = "LeyenPrefixToolsGroup";
        type Type = super::PrefixToolsGroup;
        type ParentType = adw::PreferencesGroup;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PrefixToolsGroup {}
    impl WidgetImpl for PrefixToolsGroup {}
    impl PreferencesGroupImpl for PrefixToolsGroup {}

    #[gtk4::template_callbacks]
    impl PrefixToolsGroup {
        #[template_callback]
        fn on_winecfg(&self, _row: &adw::ActionRow) {
            let obj = self.obj();
            if let Some(target) = obj.target() {
                obj.run(
                    async move { prefix_tools::run_winecfg(&target.prefix, &target.proton).await },
                );
            }
        }

        #[template_callback]
        fn on_regedit(&self, _row: &adw::ActionRow) {
            let obj = self.obj();
            if let Some(target) = obj.target() {
                obj.run(
                    async move { prefix_tools::run_regedit(&target.prefix, &target.proton).await },
                );
            }
        }

        #[template_callback]
        fn on_dependencies(&self, _row: &adw::ActionRow) {
            let obj = self.obj();
            let Some(target) = obj.target() else {
                return;
            };
            glib::spawn_future_local(glib::clone!(
                #[weak]
                obj,
                async move {
                    match DependenciesPage::open(&target.prefix, &target.proton).await {
                        Ok(page) => obj.push(&page),
                        Err(reason) => obj.toast(&reason),
                    }
                }
            ));
        }

        #[template_callback]
        fn on_run(&self, _row: &adw::ActionRow) {
            let obj = self.obj();
            let Some(target) = obj.target() else {
                return;
            };
            let parent = obj.root().and_downcast::<gtk4::Window>();
            glib::spawn_future_local(glib::clone!(
                #[weak]
                obj,
                async move {
                    if let Some(message) =
                        prefix_tools::pick_and_run(parent.as_ref(), &target.prefix, &target.proton)
                            .await
                    {
                        obj.toast(&message);
                    }
                }
            ));
        }
    }
}

glib::wrapper! {
    pub struct PrefixToolsGroup(ObjectSubclass<imp::PrefixToolsGroup>)
        @extends adw::PreferencesGroup, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl PrefixToolsGroup {
    /// Where the tools act, asked for each time one is used: the dialog's rows may
    /// have changed since. An `Err` is shown instead of running the tool.
    pub fn set_target(&self, target: impl Fn() -> Result<ToolTarget, String> + 'static) {
        self.imp().target.replace(Some(Box::new(target)));
    }

    pub fn show_tools(&self) {
        self.set_tools_visible(true);
    }

    /// Replaces the tools with a note on where this prefix is managed.
    pub fn show_notice(&self, title: &str, subtitle: &str, icon_name: &str) {
        let imp = self.imp();
        imp.notice_row.set_title(title);
        imp.notice_row.set_subtitle(subtitle);
        imp.notice_icon.set_icon_name(Some(icon_name));
        self.set_tools_visible(false);
    }

    fn set_tools_visible(&self, visible: bool) {
        let imp = self.imp();
        imp.notice_row.set_visible(!visible);
        for row in [
            &imp.winecfg_row,
            &imp.regedit_row,
            &imp.dependencies_row,
            &imp.run_row,
        ] {
            row.set_visible(visible);
        }
    }

    fn target(&self) -> Option<ToolTarget> {
        let result = self.imp().target.borrow().as_ref().map(|target| target())?;
        result.map_err(|reason| self.toast(&reason)).ok()
    }

    fn run(&self, tool: impl std::future::Future<Output = String> + 'static) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = group)]
            self,
            async move {
                let message = tool.await;
                group.toast(&message);
            }
        ));
    }

    /// Shows the page over the dialog this group is in.
    fn push(&self, page: &DependenciesPage) {
        if let Some(dialog) = self.ancestor(adw::PreferencesDialog::static_type()) {
            dialog
                .downcast::<adw::PreferencesDialog>()
                .expect("an ancestor of that type")
                .push_subpage(page);
        } else if let Some(view) = self.ancestor(adw::NavigationView::static_type()) {
            view.downcast::<adw::NavigationView>()
                .expect("an ancestor of that type")
                .push(page);
        }
    }

    fn toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        if let Some(dialog) = self.ancestor(adw::PreferencesDialog::static_type()) {
            dialog
                .downcast::<adw::PreferencesDialog>()
                .expect("an ancestor of that type")
                .add_toast(toast);
        } else if let Some(overlay) = self.ancestor(adw::ToastOverlay::static_type()) {
            overlay
                .downcast::<adw::ToastOverlay>()
                .expect("an ancestor of that type")
                .add_toast(toast);
        } else {
            log::warn!("{message}");
        }
    }
}

/// The title of the notice shown when a game or a group uses the default prefix.
pub fn managed_by_preferences() -> (String, String) {
    (
        gettext("Managed by global preferences"),
        gettext("Use Preferences to manage dependencies or run a program in the default prefix."),
    )
}
