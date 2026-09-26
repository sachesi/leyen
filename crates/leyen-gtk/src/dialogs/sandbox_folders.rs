//! The Extra Folders row of the preferences and of the game and group dialogs.

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_model::i18n::gettext;
use leyen_model::models::SandboxFolder;
use libadwaita as adw;

use super::parent_window;

fn access_label(writable: bool) -> String {
    if writable {
        gettext("Writable")
    } else {
        gettext("Read-only")
    }
}

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/sandbox_folders_row.ui")]
    pub struct SandboxFoldersRow {
        #[template_child]
        pub add_row: TemplateChild<adw::ActionRow>,
        pub folders: RefCell<Vec<SandboxFolder>>,
        pub rows: RefCell<Vec<adw::ActionRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SandboxFoldersRow {
        const NAME: &'static str = "LeyenSandboxFoldersRow";
        type Type = super::SandboxFoldersRow;
        type ParentType = adw::ExpanderRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SandboxFoldersRow {}
    impl WidgetImpl for SandboxFoldersRow {}
    impl ListBoxRowImpl for SandboxFoldersRow {}
    impl PreferencesRowImpl for SandboxFoldersRow {}
    impl ExpanderRowImpl for SandboxFoldersRow {}

    #[gtk4::template_callbacks]
    impl SandboxFoldersRow {
        #[template_callback]
        fn on_add(&self, _row: &adw::ActionRow) {
            let obj = self.obj().clone();
            glib::spawn_future_local(async move {
                let dialog = gtk4::FileDialog::builder()
                    .title(gettext("Select Folder to Share"))
                    .build();
                if let Ok(file) = dialog
                    .select_folder_future(parent_window(&obj).as_ref())
                    .await
                    && let Some(path) = file.path()
                {
                    obj.add_folder(&path.to_string_lossy());
                }
            });
        }
    }
}

glib::wrapper! {
    pub struct SandboxFoldersRow(ObjectSubclass<imp::SandboxFoldersRow>)
        @extends adw::ExpanderRow, adw::PreferencesRow, gtk4::ListBoxRow, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Actionable;
}

impl Default for SandboxFoldersRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SandboxFoldersRow {
    pub fn set_folders(&self, folders: &[SandboxFolder]) {
        self.imp().folders.replace(folders.to_vec());
        self.rebuild();
    }

    pub fn folders(&self) -> Vec<SandboxFolder> {
        self.imp()
            .folders
            .borrow()
            .iter()
            .filter(|folder| !folder.path.trim().is_empty())
            .cloned()
            .collect()
    }

    fn add_folder(&self, path: &str) {
        let path = path.trim().to_string();
        if path.is_empty() {
            return;
        }
        {
            let mut folders = self.imp().folders.borrow_mut();
            if folders.iter().any(|folder| folder.path == path) {
                return;
            }
            folders.push(SandboxFolder {
                path,
                writable: false,
            });
        }
        self.rebuild();
    }

    fn remove_folder(&self, index: usize) {
        {
            let mut folders = self.imp().folders.borrow_mut();
            if index >= folders.len() {
                return;
            }
            folders.remove(index);
        }
        self.rebuild();
    }

    fn set_writable(&self, index: usize, writable: bool) {
        if let Some(folder) = self.imp().folders.borrow_mut().get_mut(index) {
            folder.writable = writable;
        }
    }

    fn rebuild(&self) {
        let imp = self.imp();
        for row in imp.rows.borrow_mut().drain(..) {
            self.remove(&row);
        }
        self.remove(&imp.add_row.get());

        let folders = imp.folders.borrow().clone();
        for (index, folder) in folders.iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(&folder.path)
                .use_markup(false)
                .subtitle(access_label(folder.writable))
                .build();

            let writable = gtk4::Switch::builder()
                .active(folder.writable)
                .valign(gtk4::Align::Center)
                .tooltip_text(gettext("Let the game write to this folder"))
                .build();
            writable.update_property(&[gtk4::accessible::Property::Label(&gettext("Writable"))]);
            // Weak: the switch sits inside both, and a strong reference from its
            // handler would keep the whole row alive after the dialog closes.
            writable.connect_active_notify(glib::clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak(rename_to = access_row)]
                row,
                move |switch| {
                    obj.set_writable(index, switch.is_active());
                    access_row.set_subtitle(&access_label(switch.is_active()));
                }
            ));
            row.add_suffix(&writable);

            let remove = gtk4::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(gettext("Stop sharing this folder"))
                .valign(gtk4::Align::Center)
                .build();
            remove.add_css_class("flat");
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| obj.remove_folder(index)
            ));
            row.add_suffix(&remove);

            self.add_row(&row);
            imp.rows.borrow_mut().push(row);
        }

        self.add_row(&imp.add_row.get());
        self.set_subtitle(&if folders.is_empty() {
            gettext(
                "Folders to share besides the game's own: a data folder elsewhere, mods, saves you keep apart.",
            )
        } else {
            leyen_model::i18n::ngettext(
                "{} folder shared",
                "{} folders shared",
                folders.len() as u32,
            )
            .replacen("{}", &folders.len().to_string(), 1)
        });
    }
}
