use std::path::Path;
use image::imageops::FilterType;
use gtk4::prelude::*;
use crate::ui::{LIBRARY_ICON_SIZE, LIBRARY_ICON_CORNER_RADIUS};

pub fn build_library_icon(
    icon_path: Option<std::path::PathBuf>,
    fallback_icon: &str,
    valign: gtk4::Align,
    is_running: bool,
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

    let icon_widget: gtk4::Widget = if let Some(path) = icon_path.as_deref().filter(|path| path.is_file())
        && let Some(icon) = build_scaled_art_icon(path)
    {
        icon.upcast()
    } else if fallback_icon == "folder"
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

    wrapper.append(&icon_widget);
    overlay.set_child(Some(&wrapper));

    if is_running {
        let badge = gtk4::Box::builder()
            .css_classes(["running-badge"])
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::End)
            .build();
        badge.set_size_request(12, 12);
        overlay.add_overlay(&badge);
    }

    overlay.upcast()
}

fn build_scaled_art_icon(path: &Path) -> Option<gtk4::Picture> {
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
    let stride = usize::try_from(width).ok()?.checked_mul(4)?;
    let bytes = gtk4::glib::Bytes::from_owned(rgba.into_raw());
    let texture = gtk4::gdk::MemoryTexture::new(
        width,
        height,
        gtk4::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );

    let picture = gtk4::Picture::for_paintable(&texture);
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(LIBRARY_ICON_SIZE, LIBRARY_ICON_SIZE);
    picture.set_halign(gtk4::Align::Fill);
    picture.set_valign(gtk4::Align::Fill);
    picture.add_css_class("library-icon-media");

    Some(picture)
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
