//! AIDAW's own line icons (assets/icons/*.svg), drawn in one style so the
//! toolbar doesn't mix emoji from different fonts.
//!
//! The SVGs are white; [`image`] tints them to any colour.

use eframe::egui::{self, include_image, Color32, ImageSource, Rect, WidgetText};

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    ArrowDown,
    ArrowUp,
    Cap,
    ChevronDown,
    ChevronLeft,
    ChevronUp,
    ChevronsDown,
    ChevronsUp,
    Clipboard,
    Folder,
    Help,
    Home,
    Keyboard,
    Magnet,
    Mute,
    Pause,
    Play,
    Plus,
    Redo,
    Save,
    Sliders,
    Sparkles,
    Sprout,
    Stop,
    Sync,
    Trash,
    Undo,
    Volume,
}

impl Icon {
    pub fn source(self) -> ImageSource<'static> {
        match self {
            Icon::ArrowDown => include_image!("../assets/icons/arrow-down.svg"),
            Icon::ArrowUp => include_image!("../assets/icons/arrow-up.svg"),
            Icon::Cap => include_image!("../assets/icons/cap.svg"),
            Icon::ChevronDown => include_image!("../assets/icons/chevron-down.svg"),
            Icon::ChevronLeft => include_image!("../assets/icons/chevron-left.svg"),
            Icon::ChevronUp => include_image!("../assets/icons/chevron-up.svg"),
            Icon::ChevronsDown => include_image!("../assets/icons/chevrons-down.svg"),
            Icon::ChevronsUp => include_image!("../assets/icons/chevrons-up.svg"),
            Icon::Clipboard => include_image!("../assets/icons/clipboard.svg"),
            Icon::Folder => include_image!("../assets/icons/folder.svg"),
            Icon::Help => include_image!("../assets/icons/help.svg"),
            Icon::Home => include_image!("../assets/icons/home.svg"),
            Icon::Keyboard => include_image!("../assets/icons/keyboard.svg"),
            Icon::Magnet => include_image!("../assets/icons/magnet.svg"),
            Icon::Mute => include_image!("../assets/icons/mute.svg"),
            Icon::Pause => include_image!("../assets/icons/pause.svg"),
            Icon::Play => include_image!("../assets/icons/play.svg"),
            Icon::Plus => include_image!("../assets/icons/plus.svg"),
            Icon::Redo => include_image!("../assets/icons/redo.svg"),
            Icon::Save => include_image!("../assets/icons/save.svg"),
            Icon::Sliders => include_image!("../assets/icons/sliders.svg"),
            Icon::Sparkles => include_image!("../assets/icons/sparkles.svg"),
            Icon::Sprout => include_image!("../assets/icons/sprout.svg"),
            Icon::Stop => include_image!("../assets/icons/stop.svg"),
            Icon::Sync => include_image!("../assets/icons/sync.svg"),
            Icon::Trash => include_image!("../assets/icons/trash.svg"),
            Icon::Undo => include_image!("../assets/icons/undo.svg"),
            Icon::Volume => include_image!("../assets/icons/volume.svg"),
        }
    }
}

/// The icon at `size` points, tinted `color`.
pub fn image(icon: Icon, size: f32, color: Color32) -> egui::Image<'static> {
    egui::Image::new(icon.source())
        .fit_to_exact_size(egui::vec2(size, size))
        .tint(color)
}

/// A normal button with a 16 pt icon in front of its text.
pub fn button(icon: Icon, text: impl Into<WidgetText>) -> egui::Button<'static> {
    egui::Button::image_and_text(image(icon, 16.0, theme::TEXT), text)
}

/// Same as [`button`], with icon and text in `color` (warnings, errors).
pub fn button_colored(icon: Icon, text: &str, color: Color32) -> egui::Button<'static> {
    egui::Button::image_and_text(
        image(icon, 16.0, color),
        egui::RichText::new(text).color(color),
    )
}

/// An icon-only button (the tooltip says what it does).
pub fn icon_button(icon: Icon, size: f32) -> egui::Button<'static> {
    egui::Button::image(image(icon, size, theme::TEXT))
}

/// Paint the icon into `rect` on a custom-drawn widget.
pub fn paint(ui: &egui::Ui, rect: Rect, icon: Icon, color: Color32) {
    image(icon, rect.width(), color).paint_at(ui, rect);
}

/// A dim inline icon, e.g. next to a slider.
pub fn label(ui: &mut egui::Ui, icon: Icon) -> egui::Response {
    ui.add(image(icon, 15.0, theme::TEXT_DIM))
}
