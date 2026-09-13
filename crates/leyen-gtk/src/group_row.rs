//! A group in the library: how many games it holds, how many run, and when one was
//! last played.

use std::cell::{Cell, RefCell};

use adw::subclass::prelude::*;
use gtk4::glib;
use gtk4::prelude::*;
use leyen_model::i18n::ngettext;
use leyen_model::icons::group_icon_path;
use leyen_model::models::GameGroup;
use libadwaita as adw;

use crate::format;
use crate::library_icon::LibraryIcon;
use crate::window::RunningGameMap;

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/group_row.ui")]
    pub struct GroupRow {
        #[template_child]
        pub icon: TemplateChild<LibraryIcon>,
        #[template_child]
        pub title_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub count_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub running_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub status_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub edit_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub delete_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub open_button: TemplateChild<gtk4::Button>,
        pub group_id: RefCell<String>,
        pub title: RefCell<String>,
        /// When the first of its running games started.
        pub running_since: Cell<Option<u64>>,
        pub last_played: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GroupRow {
        const NAME: &'static str = "LeyenGroupRow";
        type Type = super::GroupRow;
        type ParentType = gtk4::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            LibraryIcon::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for GroupRow {}
    impl WidgetImpl for GroupRow {}
    impl ListBoxRowImpl for GroupRow {}
}

glib::wrapper! {
    pub struct GroupRow(ObjectSubclass<imp::GroupRow>)
        @extends gtk4::ListBoxRow, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for GroupRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl GroupRow {
    pub fn update(&self, group: &GameGroup, running: &RunningGameMap) {
        let imp = self.imp();
        imp.title_label.set_label(&group.title);
        self.update_property(&[gtk4::accessible::Property::Label(&group.title)]);
        imp.icon.set_path(group_icon_path(&group.id));

        let count = group.games.len() as u32;
        imp.count_label
            .set_label(&ngettext("{} game", "{} games", count).replacen(
                "{}",
                &count.to_string(),
                1,
            ));

        let running_games: Vec<u64> = group
            .games
            .iter()
            .filter_map(|game| running.get(&game.id))
            .map(|snapshot| snapshot.started_at_epoch_seconds)
            .collect();
        imp.running_label.set_visible(!running_games.is_empty());
        let running_count = running_games.len() as u32;
        imp.running_label.set_label(
            // Translators: how many games of a group are running.
            &ngettext("{} running", "{} running", running_count).replacen(
                "{}",
                &running_count.to_string(),
                1,
            ),
        );
        imp.running_since.set(running_games.iter().copied().min());
        imp.last_played.set(
            group
                .games
                .iter()
                .map(|game| game.last_played_epoch_seconds)
                .max()
                .unwrap_or(0),
        );

        let target = group.id.to_variant();
        imp.edit_button.set_action_target_value(Some(&target));
        imp.delete_button.set_action_target_value(Some(&target));
        imp.open_button.set_action_target_value(Some(&target));

        imp.group_id.replace(group.id.clone());
        imp.title.replace(group.title.clone());

        let (add, remove) = if imp.running_since.get().is_some() {
            ("accent", "dim-label")
        } else {
            ("dim-label", "accent")
        };
        imp.status_label.add_css_class(add);
        imp.status_label.remove_css_class(remove);
        self.tick();
    }

    pub fn tick(&self) {
        let imp = self.imp();
        let status = match imp.running_since.get() {
            Some(started_at) => format::running_for(started_at),
            None => format::last_played(imp.last_played.get()),
        };
        imp.status_label.set_label(&status);
    }

    pub fn group_id(&self) -> String {
        self.imp().group_id.borrow().clone()
    }

    pub fn title(&self) -> String {
        self.imp().title.borrow().clone()
    }
}
