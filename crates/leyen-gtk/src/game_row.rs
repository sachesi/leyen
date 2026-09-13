//! A game in the library or in a group: its icon, playtime and last session, with
//! edit, delete and launch or stop.

use std::cell::{Cell, RefCell};

use adw::subclass::prelude::*;
use gtk4::glib;
use gtk4::prelude::*;
use leyen_model::i18n::gettext;
use leyen_model::icons::game_icon_path;
use leyen_model::models::Game;
use libadwaita as adw;

use crate::format;
use crate::library_icon::LibraryIcon;

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/game_row.ui")]
    pub struct GameRow {
        #[template_child]
        pub icon: TemplateChild<LibraryIcon>,
        #[template_child]
        pub title_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub playtime_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub status_label: TemplateChild<gtk4::Label>,
        #[template_child]
        pub edit_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub delete_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub play_button: TemplateChild<gtk4::Button>,
        pub game: RefCell<Game>,
        /// The group the game is in, id and title, when it is in one.
        pub group: RefCell<Option<(String, String)>>,
        pub running_since: Cell<Option<u64>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GameRow {
        const NAME: &'static str = "LeyenGameRow";
        type Type = super::GameRow;
        type ParentType = gtk4::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            LibraryIcon::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for GameRow {}
    impl WidgetImpl for GameRow {}
    impl ListBoxRowImpl for GameRow {}
}

glib::wrapper! {
    pub struct GameRow(ObjectSubclass<imp::GameRow>)
        @extends gtk4::ListBoxRow, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Actionable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Default for GameRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl GameRow {
    /// Shows `game`, inside the group `group` (id, title) when it is in one, running
    /// since `running_since` when it runs.
    pub fn update(&self, game: &Game, group: Option<(&str, &str)>, running_since: Option<u64>) {
        let imp = self.imp();
        imp.title_label.set_label(&game.title);
        self.update_property(&[gtk4::accessible::Property::Label(&game.title)]);
        imp.playtime_label
            .set_label(&format::playtime(game.playtime_seconds));
        imp.icon.set_path(game_icon_path(&game.id));

        let target = game.id.to_variant();
        imp.edit_button.set_action_target_value(Some(&target));
        imp.delete_button.set_action_target_value(Some(&target));
        imp.play_button.set_action_target_value(Some(&target));

        imp.game.replace(game.clone());
        imp.group
            .replace(group.map(|(id, title)| (id.to_string(), title.to_string())));
        self.set_running_since(running_since);
    }

    pub fn set_running_since(&self, running_since: Option<u64>) {
        let imp = self.imp();
        imp.running_since.set(running_since);
        let running = running_since.is_some();

        if running {
            self.add_css_class("running");
        } else {
            self.remove_css_class("running");
        }
        let (add, remove) = if running {
            ("accent", "dim-label")
        } else {
            ("dim-label", "accent")
        };
        imp.status_label.add_css_class(add);
        imp.status_label.remove_css_class(remove);
        self.tick();

        let (icon, tooltip, add, remove) = if running {
            (
                "media-playback-stop-symbolic",
                gettext("Stop Game"),
                "destructive-action",
                "suggested-action",
            )
        } else {
            (
                "media-playback-start-symbolic",
                gettext("Launch Game"),
                "suggested-action",
                "destructive-action",
            )
        };
        imp.play_button.set_icon_name(icon);
        imp.play_button.set_tooltip_text(Some(&tooltip));
        imp.play_button.add_css_class(add);
        imp.play_button.remove_css_class(remove);
    }

    /// Advances "Running for …"; the last session is shown when the game is idle.
    pub fn tick(&self) {
        let imp = self.imp();
        let status = match imp.running_since.get() {
            Some(started_at) => format::running_for(started_at),
            None => format::last_played(imp.game.borrow().last_played_epoch_seconds),
        };
        imp.status_label.set_label(&status);
    }

    pub fn game(&self) -> Game {
        self.imp().game.borrow().clone()
    }

    pub fn game_id(&self) -> String {
        self.imp().game.borrow().id.clone()
    }

    pub fn title(&self) -> String {
        self.imp().game.borrow().title.clone()
    }

    pub fn is_running(&self) -> bool {
        self.imp().running_since.get().is_some()
    }

    pub fn in_group(&self) -> bool {
        self.imp().group.borrow().is_some()
    }

    /// Whether a search for `query` (lowercase) finds the game: by its title or by the
    /// title of its group.
    pub fn matches(&self, query: &str) -> bool {
        self.imp()
            .game
            .borrow()
            .title
            .to_lowercase()
            .contains(query)
            || self
                .imp()
                .group
                .borrow()
                .as_ref()
                .is_some_and(|(_, title)| title.to_lowercase().contains(query))
    }
}
