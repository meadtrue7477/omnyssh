//! Built-in colour themes.
//!
//! A [`Theme`] is a flat collection of named [`ratatui::style::Color`] values
//! that the UI layer reads at render time.  There are four built-in palettes:
//!
//! | Name      | Character                               |
//! |-----------|---------------------------------------- |
//! | `default` | Neutral blue/cyan — matches most terms  |
//! | `dracula` | Purple/pink/green — Dracula palette     |
//! | `nord`    | Arctic blues/teals — Nord palette       |
//! | `gruvbox` | Warm amber/orange — Gruvbox retro       |
//!
//! Usage:
//! ```ignore
//! let theme = Theme::from_name("dracula");
//! let badge_style = Style::default().fg(theme.key_badge_fg).bg(theme.key_badge_bg);
//! ```

use ratatui::style::Color;

/// A colour palette used throughout the TUI.
///
/// All render functions receive `&ViewState`, which carries the active theme,
/// so the palette flows through without any extra parameters.
#[derive(Debug, Clone)]
pub struct Theme {
    // Core colors
    /// Primary accent — selected card borders, active tab highlights.
    pub accent: Color,
    /// Secondary highlight — active pane borders, emphasis text.
    pub highlight: Color,
    /// Default inactive border colour.
    pub border: Color,
    /// Background of the selected list item / card header.
    pub selected_bg: Color,
    /// Screen / section title colour.
    pub title: Color,

    // Status bar colors
    /// Background of key-hint badges in the status bar (`[q]`).
    pub key_badge_bg: Color,
    /// Foreground of key-hint badges (high-contrast, usually near-black).
    pub key_badge_fg: Color,
    /// Foreground of the descriptive text after each badge.
    pub hint_fg: Color,
    /// Foreground of `│` separators in the status bar.
    pub separator_fg: Color,

    // Popup and form colors
    /// Popup border color (general purpose).
    pub popup_border: Color,
    /// Success popup border (confirmations, successful operations).
    pub success_border: Color,
    /// Warning popup border (important actions).
    pub warning_border: Color,
    /// Error/danger popup border (deletions, errors).
    pub danger_border: Color,
    /// Form input background (focused).
    pub form_focused_bg: Color,
    /// Form input foreground (focused).
    pub form_focused_fg: Color,
    /// Form input background (normal).
    pub form_normal_bg: Color,
    /// Form input foreground (normal).
    pub form_normal_fg: Color,
    /// Form label color.
    pub form_label: Color,
    /// Form label color (focused field).
    pub form_label_focused: Color,

    // Text colors
    /// Primary text color.
    pub text_primary: Color,
    /// Secondary/dimmed text color.
    pub text_secondary: Color,
    /// Muted/disabled text color.
    pub text_muted: Color,
    /// Error text color.
    pub text_error: Color,
    /// Success text color.
    pub text_success: Color,
    /// Warning text color.
    pub text_warning: Color,

    // File manager colors
    /// Directory color in file listings.
    pub file_dir: Color,
    /// Regular file color in file listings.
    pub file_regular: Color,
    /// Marked/selected file color.
    pub file_marked: Color,
}

impl Theme {
    // ------------------------------------------------------------------
    // Built-in palettes
    // ------------------------------------------------------------------

    /// Neutral blue/cyan palette — blends with most terminal colour schemes.
    pub fn default_theme() -> Self {
        Self {
            // Core colors
            accent: Color::Cyan,
            highlight: Color::LightCyan,
            border: Color::DarkGray,
            selected_bg: Color::DarkGray,
            title: Color::White,

            // Status bar colors
            key_badge_bg: Color::Cyan,
            key_badge_fg: Color::Black,
            hint_fg: Color::Gray,
            separator_fg: Color::DarkGray,

            // Popup and form colors
            popup_border: Color::Cyan,
            success_border: Color::Green,
            warning_border: Color::Yellow,
            danger_border: Color::Red,
            form_focused_bg: Color::Cyan,
            form_focused_fg: Color::Black,
            form_normal_bg: Color::DarkGray,
            form_normal_fg: Color::White,
            form_label: Color::Gray,
            form_label_focused: Color::Cyan,

            // Text colors
            text_primary: Color::White,
            text_secondary: Color::Gray,
            text_muted: Color::DarkGray,
            text_error: Color::Red,
            text_success: Color::Green,
            text_warning: Color::Yellow,

            // File manager colors
            file_dir: Color::Cyan,
            file_regular: Color::Gray,
            file_marked: Color::Yellow,
        }
    }

    /// Dracula — purple / pink / green.
    ///
    /// Reference: <https://draculatheme.com/contribute>
    pub fn dracula() -> Self {
        Self {
            // Core colors
            accent: Color::Rgb(189, 147, 249),    // purple
            highlight: Color::Rgb(255, 121, 198), // pink
            border: Color::Rgb(68, 71, 90),       // selection (dark bg)
            selected_bg: Color::Rgb(68, 71, 90),
            title: Color::Rgb(80, 250, 123), // green

            // Status bar colors
            key_badge_bg: Color::Rgb(189, 147, 249), // purple
            key_badge_fg: Color::Rgb(40, 42, 54),    // background
            hint_fg: Color::Rgb(248, 248, 242),      // foreground
            separator_fg: Color::Rgb(68, 71, 90),

            // Popup and form colors
            popup_border: Color::Rgb(189, 147, 249), // purple
            success_border: Color::Rgb(80, 250, 123), // green
            warning_border: Color::Rgb(241, 250, 140), // yellow
            danger_border: Color::Rgb(255, 85, 85),  // red
            form_focused_bg: Color::Rgb(189, 147, 249),
            form_focused_fg: Color::Rgb(40, 42, 54),
            form_normal_bg: Color::Rgb(68, 71, 90),
            form_normal_fg: Color::Rgb(248, 248, 242),
            form_label: Color::Rgb(98, 114, 164), // comment
            form_label_focused: Color::Rgb(189, 147, 249),

            // Text colors
            text_primary: Color::Rgb(248, 248, 242), // foreground
            text_secondary: Color::Rgb(98, 114, 164), // comment
            text_muted: Color::Rgb(68, 71, 90),
            text_error: Color::Rgb(255, 85, 85),     // red
            text_success: Color::Rgb(80, 250, 123),  // green
            text_warning: Color::Rgb(241, 250, 140), // yellow

            // File manager colors
            file_dir: Color::Rgb(139, 233, 253), // cyan
            file_regular: Color::Rgb(98, 114, 164),
            file_marked: Color::Rgb(241, 250, 140), // yellow
        }
    }

    /// Nord — arctic blues and teals.
    ///
    /// Reference: <https://www.nordtheme.com/docs/colors-and-palettes>
    pub fn nord() -> Self {
        Self {
            // Core colors
            accent: Color::Rgb(136, 192, 208), // nord8  (frost light)
            highlight: Color::Rgb(129, 161, 193), // nord9  (frost mid)
            border: Color::Rgb(59, 66, 82),    // nord1  (dark bg)
            selected_bg: Color::Rgb(67, 76, 94), // nord2
            title: Color::Rgb(143, 188, 187),  // nord7  (teal)

            // Status bar colors
            key_badge_bg: Color::Rgb(136, 192, 208), // nord8
            key_badge_fg: Color::Rgb(46, 52, 64),    // nord0  (darkest bg)
            hint_fg: Color::Rgb(216, 222, 233),      // nord4  (light fg)
            separator_fg: Color::Rgb(76, 86, 106),   // nord3

            // Popup and form colors
            popup_border: Color::Rgb(136, 192, 208), // nord8
            success_border: Color::Rgb(163, 190, 140), // nord14 (green)
            warning_border: Color::Rgb(235, 203, 139), // nord13 (yellow)
            danger_border: Color::Rgb(191, 97, 106), // nord11 (red)
            form_focused_bg: Color::Rgb(136, 192, 208),
            form_focused_fg: Color::Rgb(46, 52, 64),
            form_normal_bg: Color::Rgb(67, 76, 94),
            form_normal_fg: Color::Rgb(216, 222, 233),
            form_label: Color::Rgb(76, 86, 106),
            form_label_focused: Color::Rgb(136, 192, 208),

            // Text colors
            text_primary: Color::Rgb(236, 239, 244), // nord6 (snow)
            text_secondary: Color::Rgb(216, 222, 233), // nord4
            text_muted: Color::Rgb(76, 86, 106),     // nord3
            text_error: Color::Rgb(191, 97, 106),    // nord11 (red)
            text_success: Color::Rgb(163, 190, 140), // nord14 (green)
            text_warning: Color::Rgb(235, 203, 139), // nord13 (yellow)

            // File manager colors
            file_dir: Color::Rgb(136, 192, 208), // nord8
            file_regular: Color::Rgb(216, 222, 233),
            file_marked: Color::Rgb(235, 203, 139), // nord13
        }
    }

    /// Gruvbox — warm amber/orange retro palette.
    ///
    /// Reference: <https://github.com/morhetz/gruvbox>
    pub fn gruvbox() -> Self {
        Self {
            // Core colors
            accent: Color::Rgb(250, 189, 47),    // bright yellow
            highlight: Color::Rgb(254, 128, 25), // bright orange
            border: Color::Rgb(80, 73, 69),      // bg2
            selected_bg: Color::Rgb(80, 73, 69),
            title: Color::Rgb(184, 187, 38), // bright green

            // Status bar colors
            key_badge_bg: Color::Rgb(215, 153, 33), // yellow
            key_badge_fg: Color::Rgb(40, 40, 40),   // bg hard
            hint_fg: Color::Rgb(235, 219, 178),     // fg1
            separator_fg: Color::Rgb(102, 92, 84),  // bg4

            // Popup and form colors
            popup_border: Color::Rgb(215, 153, 33),   // yellow
            success_border: Color::Rgb(184, 187, 38), // green
            warning_border: Color::Rgb(250, 189, 47), // bright yellow
            danger_border: Color::Rgb(251, 73, 52),   // red
            form_focused_bg: Color::Rgb(215, 153, 33),
            form_focused_fg: Color::Rgb(40, 40, 40),
            form_normal_bg: Color::Rgb(80, 73, 69),
            form_normal_fg: Color::Rgb(235, 219, 178),
            form_label: Color::Rgb(102, 92, 84),
            form_label_focused: Color::Rgb(215, 153, 33),

            // Text colors
            text_primary: Color::Rgb(251, 241, 199),   // fg0
            text_secondary: Color::Rgb(213, 196, 161), // fg2
            text_muted: Color::Rgb(102, 92, 84),       // bg4
            text_error: Color::Rgb(251, 73, 52),       // red
            text_success: Color::Rgb(184, 187, 38),    // green
            text_warning: Color::Rgb(250, 189, 47),    // bright yellow

            // File manager colors
            file_dir: Color::Rgb(131, 165, 152), // aqua
            file_regular: Color::Rgb(213, 196, 161),
            file_marked: Color::Rgb(250, 189, 47), // bright yellow
        }
    }

    // ------------------------------------------------------------------
    // Factory
    // ------------------------------------------------------------------

    /// Constructs the appropriate theme from a config name string.
    ///
    /// | `name`      | Result               |
    /// |-------------|----------------------|
    /// | `"default"` | [`Self::default_theme`] |
    /// | `"dracula"` | [`Self::dracula`]    |
    /// | `"nord"`    | [`Self::nord`]       |
    /// | `"gruvbox"` | [`Self::gruvbox`]    |
    /// | anything else | [`Self::default_theme`] |
    pub fn from_name(name: &str) -> Self {
        match name {
            "dracula" => Self::dracula(),
            "nord" => Self::nord(),
            "gruvbox" => Self::gruvbox(),
            _ => Self::default_theme(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::default_theme()
    }
}
