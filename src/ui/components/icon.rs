use crate::ui::LIBRARY_ICON_SIZE;
use gtk4::prelude::*;
use image::imageops::FilterType;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Cached processed icon, keyed by absolute path. Avoids re-decoding and
/// re-resizing the same image on every library rebuild. Textures are GPU-shared
/// across all `Picture` widgets that reference them, so this is cheap to clone.
struct CachedIcon {
    mtime_epoch_seconds: u64,
    len: u64,
    texture: gtk4::gdk::MemoryTexture,
}

thread_local! {
    static ICON_CACHE: RefCell<HashMap<std::path::PathBuf, CachedIcon>> =
        RefCell::new(HashMap::new());
}

/// Cheap `(mtime, len)` stamp used to invalidate the cache when the file changes.
fn icon_file_stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((mtime, meta.len()))
}

/// Returns a cached texture for `path` if present and still matching `stamp`.
fn cached_texture(path: &Path, stamp: (u64, u64)) -> Option<gtk4::gdk::MemoryTexture> {
    ICON_CACHE.with(|cache| {
        cache.borrow().get(path).and_then(|entry| {
            (entry.mtime_epoch_seconds == stamp.0 && entry.len == stamp.1)
                .then(|| entry.texture.clone())
        })
    })
}

fn store_texture(path: &Path, stamp: (u64, u64), texture: &gtk4::gdk::MemoryTexture) {
    ICON_CACHE.with(|cache| {
        cache.borrow_mut().insert(
            path.to_path_buf(),
            CachedIcon {
                mtime_epoch_seconds: stamp.0,
                len: stamp.1,
                texture: texture.clone(),
            },
        );
    });
}

pub fn build_library_icon(
    icon_path: Option<std::path::PathBuf>,
    fallback_icon: &str,
    valign: gtk4::Align,
) -> gtk4::Widget {
    let overlay = gtk4::Overlay::builder()
        .halign(gtk4::Align::Center)
        .valign(valign)
        .build();

    let wrapper = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .build();
    wrapper.set_size_request(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE);
    wrapper.set_overflow(gtk4::Overflow::Hidden);
    wrapper.add_css_class("library-icon-frame");

    let fallback_widget: gtk4::Widget = if fallback_icon == "folder"
        && let Some(icon) = build_themed_folder_icon()
    {
        icon.upcast()
    } else {
        gtk4::Image::builder()
            .icon_name(fallback_icon)
            .pixel_size(LIBRARY_ICON_SIZE)
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .build()
            .upcast()
    };

    wrapper.append(&fallback_widget);
    overlay.set_child(Some(&wrapper));

    if let Some(path) = icon_path {
        let stamp = icon_file_stamp(&path);

        // Cache hit — swap in the texture immediately, no decode, no thread hop.
        if let Some(stamp) = stamp
            && let Some(texture) = cached_texture(&path, stamp)
        {
            let picture = picture_from_texture(&texture);
            if let Some(old) = wrapper.first_child() {
                wrapper.remove(&old);
            }
            wrapper.append(&picture);
            return overlay.upcast();
        }

        let wrapper_clone = wrapper.clone();
        gtk4::glib::spawn_future_local(async move {
            let path_for_decode = path.clone();
            let result = tokio::task::spawn_blocking(move || process_icon_file(&path_for_decode))
                .await
                .ok()
                .flatten();

            if let Some((width, height, rgba)) = result {
                let texture = make_texture(width, height, &rgba);
                if let Some(stamp) = stamp {
                    store_texture(&path, stamp, &texture);
                }
                let picture = picture_from_texture(&texture);
                if let Some(old) = wrapper_clone.first_child() {
                    wrapper_clone.remove(&old);
                }
                wrapper_clone.append(&picture);
            }
        });
    }

    overlay.upcast()
}

fn process_icon_file(path: &Path) -> Option<(i32, i32, Vec<u8>)> {
    let image = image::open(path).ok()?;
    let image = crop_transparent_padding(image);
    let image = image.resize(
        (LIBRARY_ICON_SIZE * 2) as u32,
        (LIBRARY_ICON_SIZE * 2) as u32,
        FilterType::Lanczos3,
    );
    let rgba = image.to_rgba8();
    let width = i32::try_from(rgba.width()).ok()?;
    let height = i32::try_from(rgba.height()).ok()?;
    Some((width, height, rgba.into_raw()))
}

fn make_texture(width: i32, height: i32, rgba: &[u8]) -> gtk4::gdk::MemoryTexture {
    let stride = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .unwrap_or(0);
    let bytes = gtk4::glib::Bytes::from_owned(rgba.to_vec());
    gtk4::gdk::MemoryTexture::new(
        width,
        height,
        gtk4::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    )
}

fn picture_from_texture(texture: &gtk4::gdk::MemoryTexture) -> gtk4::Picture {
    let picture = gtk4::Picture::for_paintable(texture);
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE);
    picture.set_halign(gtk4::Align::Fill);
    picture.set_valign(gtk4::Align::Fill);
    picture.add_css_class("library-icon-media");
    picture
}

fn build_themed_folder_icon() -> Option<gtk4::Picture> {
    let display = gtk4::gdk::Display::default()?;
    let theme = gtk4::IconTheme::for_display(&display);
    let icon = theme.lookup_icon(
        "folder",
        &[],
        LIBRARY_ICON_SIZE * 2,
        1,
        gtk4::TextDirection::Ltr,
        gtk4::IconLookupFlags::empty(),
    );

    let picture = gtk4::Picture::for_paintable(&icon);
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE);
    picture.set_halign(gtk4::Align::Fill);
    picture.set_valign(gtk4::Align::Fill);
    picture.add_css_class("library-icon-media");

    Some(picture)
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
    let mut left = image.width();
    let mut top = image.height();
    let mut right = 0;
    let mut bottom = 0;
    let mut found = false;

    for (x, y, pixel) in rgba.enumerate_pixels() {
        if pixel.0[3] <= 8 {
            continue;
        }
        found = true;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }

    found.then_some((left, top, right, bottom))
}
