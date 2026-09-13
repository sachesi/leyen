//! The icon of a game or group: the managed PNG when there is one, a themed icon
//! until then. Decoding happens off the main thread and the textures are cached by
//! path, so refreshing the library repaints known icons without touching the disk.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib};
use image::imageops::FilterType;

use crate::daemon::gio_blocking;
use crate::icons::fit_square;

pub const ICON_SIZE: i32 = 56;
const ICON_CACHE_CAP: usize = 256;

/// `(mtime, len)` of an icon file, enough to notice that it was replaced.
type Stamp = (u64, u64);

thread_local! {
    static ICON_CACHE: RefCell<HashMap<PathBuf, (Stamp, gdk::MemoryTexture)>> =
        RefCell::new(HashMap::new());
}

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::LibraryIcon)]
    pub struct LibraryIcon {
        /// Shown while there is no icon file, or none that could be decoded.
        #[property(get, set = Self::set_fallback_icon_name)]
        pub fallback_icon_name: RefCell<String>,
        pub path: RefCell<Option<PathBuf>>,
        /// The stamp of the file the picture on screen came from.
        pub shown: Cell<Option<Stamp>>,
        /// Bumped on every load, so a slow decode never replaces a newer one.
        pub generation: Cell<u64>,
        pub child: RefCell<Option<gtk4::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LibraryIcon {
        const NAME: &'static str = "LeyenLibraryIcon";
        type Type = super::LibraryIcon;
        type ParentType = gtk4::Widget;

        /// Decoration: the row it sits in carries the title.
        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk4::AccessibleRole::Presentation);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for LibraryIcon {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_size_request(ICON_SIZE, ICON_SIZE);
            obj.set_halign(gtk4::Align::Center);
            obj.set_overflow(gtk4::Overflow::Hidden);
            obj.add_css_class("library-icon");
        }

        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for LibraryIcon {
        /// Always the icon size: a picture would otherwise ask for its texture's size,
        /// twice the display size for sharpness.
        fn measure(&self, _orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (ICON_SIZE, ICON_SIZE, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
        }
    }

    impl LibraryIcon {
        fn set_fallback_icon_name(&self, name: String) {
            self.fallback_icon_name.replace(name);
            if self.shown.get().is_none() {
                self.obj().show_fallback();
            }
        }
    }
}

glib::wrapper! {
    pub struct LibraryIcon(ObjectSubclass<imp::LibraryIcon>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl LibraryIcon {
    /// Shows the icon at `path`, checking the file again even when the path is the one
    /// already shown: an edited game keeps its path and gets a new file.
    pub fn set_path(&self, path: PathBuf) {
        let imp = self.imp();
        if imp.path.borrow().as_ref() != Some(&path) {
            // Paint the texture decoded last time right away; the check below
            // corrects the rare one that went stale.
            match cached_entry(&path) {
                Some((stamp, texture)) => self.show_texture(&texture, stamp),
                None => self.show_fallback(),
            }
            imp.path.replace(Some(path.clone()));
        }
        let generation = imp.generation.get().wrapping_add(1);
        imp.generation.set(generation);

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = icon)]
            self,
            async move {
                let stat_path = path.clone();
                let stamp = gio_blocking(move || file_stamp(&stat_path)).await.flatten();
                if icon.imp().generation.get() != generation {
                    return;
                }
                let Some(stamp) = stamp else {
                    icon.show_fallback();
                    return;
                };
                if icon.imp().shown.get() == Some(stamp) {
                    return;
                }
                if let Some(texture) = cached_texture(&path, stamp) {
                    icon.show_texture(&texture, stamp);
                    return;
                }
                let decode_path = path.clone();
                let decoded = gio_blocking(move || decode(&decode_path)).await.flatten();
                if icon.imp().generation.get() != generation {
                    return;
                }
                match decoded {
                    Some((width, height, rgba)) => {
                        let texture = make_texture(width, height, rgba);
                        store_texture(&path, stamp, &texture);
                        icon.show_texture(&texture, stamp);
                    }
                    None => icon.show_fallback(),
                }
            }
        ));
    }

    fn set_child(&self, child: &impl IsA<gtk4::Widget>) {
        if let Some(old) = self.imp().child.replace(Some(child.clone().upcast())) {
            old.unparent();
        }
        child.set_parent(self);
    }

    fn show_texture(&self, texture: &gdk::MemoryTexture, stamp: Stamp) {
        self.set_child(&picture(texture));
        self.imp().shown.set(Some(stamp));
    }

    fn show_fallback(&self) {
        self.imp().shown.set(None);
        let name = self.imp().fallback_icon_name.borrow().clone();
        let child: gtk4::Widget = match themed_folder_picture(&name) {
            Some(picture) => picture.upcast(),
            None => gtk4::Image::builder()
                .accessible_role(gtk4::AccessibleRole::Presentation)
                .icon_name(name)
                .pixel_size(ICON_SIZE)
                .build()
                .upcast(),
        };
        self.set_child(&child);
    }
}

fn picture(paintable: &impl IsA<gdk::Paintable>) -> gtk4::Picture {
    gtk4::Picture::builder()
        .accessible_role(gtk4::AccessibleRole::Presentation)
        .paintable(paintable)
        .content_fit(gtk4::ContentFit::Cover)
        .can_shrink(true)
        .build()
}

/// The full-colour folder of the icon theme, drawn at twice the size for a crisp
/// scale-down. Only for "folder": the other fallbacks are symbolic and draw as images.
fn themed_folder_picture(name: &str) -> Option<gtk4::Picture> {
    if name != "folder" {
        return None;
    }
    let display = gdk::Display::default()?;
    let icon = gtk4::IconTheme::for_display(&display).lookup_icon(
        name,
        &[],
        ICON_SIZE * 2,
        1,
        gtk4::TextDirection::Ltr,
        gtk4::IconLookupFlags::empty(),
    );
    Some(picture(&icon))
}

fn file_stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0);
    Some((mtime, meta.len()))
}

fn cached_entry(path: &Path) -> Option<(Stamp, gdk::MemoryTexture)> {
    ICON_CACHE.with(|cache| cache.borrow().get(path).cloned())
}

fn cached_texture(path: &Path, stamp: Stamp) -> Option<gdk::MemoryTexture> {
    cached_entry(path).and_then(|(cached, texture)| (cached == stamp).then_some(texture))
}

fn store_texture(path: &Path, stamp: Stamp, texture: &gdk::MemoryTexture) {
    ICON_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= ICON_CACHE_CAP && !cache.contains_key(path) {
            cache.clear();
        }
        cache.insert(path.to_path_buf(), (stamp, texture.clone()));
    });
}

/// Decodes the icon, trims transparent padding and makes a square of it at twice the
/// display size: a shape drawn on transparency is scaled into it whole, and artwork
/// that fills its rectangle is cropped to it from the centre and has its corners
/// rounded by the style. Runs on a worker thread.
fn decode(path: &Path) -> Option<(i32, i32, Vec<u8>)> {
    let image = image::open(path).ok()?;
    Some(square_texture_data(crop_transparent_padding(image)))
}

fn square_texture_data(image: image::DynamicImage) -> (i32, i32, Vec<u8>) {
    let size = (ICON_SIZE * 2) as u32;
    let rgba = if is_shaped(&image) {
        fit_square(&image, size, FilterType::Lanczos3)
    } else {
        image
            .resize_to_fill(size, size, FilterType::Lanczos3)
            .to_rgba8()
    };
    (ICON_SIZE * 2, ICON_SIZE * 2, rgba.into_raw())
}

/// Whether more than a tenth of the image is see-through: a logo or a round badge
/// rather than a filled rectangle. Rounded corners alone stay well under that.
fn is_shaped(image: &image::DynamicImage) -> bool {
    let rgba = image.to_rgba8();
    let clear = rgba.pixels().filter(|pixel| pixel.0[3] < 128).count();
    clear * 10 > rgba.pixels().len()
}

fn make_texture(width: i32, height: i32, rgba: Vec<u8>) -> gdk::MemoryTexture {
    let stride = usize::try_from(width).unwrap_or(0) * 4;
    gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(rgba),
        stride,
    )
}

fn crop_transparent_padding(image: image::DynamicImage) -> image::DynamicImage {
    let Some((left, top, right, bottom)) = alpha_bounds(&image) else {
        return image;
    };
    if left == 0 && top == 0 && right + 1 == image.width() && bottom + 1 == image.height() {
        return image;
    }
    image.crop_imm(left, top, right - left + 1, bottom - top + 1)
}

fn alpha_bounds(image: &image::DynamicImage) -> Option<(u32, u32, u32, u32)> {
    let rgba = image.to_rgba8();
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel.0[3] <= 8 {
            continue;
        }
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((left, top, right, bottom)) => {
                (left.min(x), top.min(y), right.max(x), bottom.max(y))
            }
        });
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::{ICON_SIZE, crop_transparent_padding, square_texture_data};
    use image::{DynamicImage, Rgba, RgbaImage};

    fn alpha(rgba: &[u8], x: i32, y: i32) -> u8 {
        rgba[((y * ICON_SIZE * 2 + x) * 4 + 3) as usize]
    }

    #[test]
    fn a_tall_logo_on_transparency_is_shown_whole() {
        let logo = RgbaImage::from_fn(60, 120, |x, y| {
            let (dx, dy) = ((x as f32 - 29.5) / 30.0, (y as f32 - 59.5) / 60.0);
            if dx * dx + dy * dy <= 1.0 {
                Rgba([200, 30, 40, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        let (width, height, rgba) =
            square_texture_data(crop_transparent_padding(DynamicImage::ImageRgba8(logo)));
        let size = ICON_SIZE * 2;
        assert_eq!((width, height), (size, size));
        // Its top and bottom are still there, and the sides are padding.
        assert!(alpha(&rgba, size / 2, 2) > 128);
        assert!(alpha(&rgba, size / 2, size - 3) > 128);
        assert_eq!(alpha(&rgba, 2, size / 2), 0);
    }

    #[test]
    fn opaque_artwork_fills_the_square() {
        let art = RgbaImage::from_pixel(200, 100, Rgba([30, 120, 200, 255]));
        let (_, _, rgba) = square_texture_data(DynamicImage::ImageRgba8(art));
        assert!(rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }
}
