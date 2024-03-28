pub mod vscode_light;
pub use vscode_light::VSCODE_LIGHT;

use crate::grid::{Style, StyleKey};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub syntax: SyntaxStyles,
    pub ui: UiStyles,
    pub diagnostic: DiagnosticStyles,
    pub hunk_old_background: Color,
    pub hunk_new_background: Color,
    pub hunk_old_emphasized_background: Color,
    pub hunk_new_emphasized_background: Color,
}
impl Theme {
    pub(crate) fn get_style(&self, source: &StyleKey) -> Style {
        match source {
            StyleKey::UiBookmark => self.ui.bookmark,
            StyleKey::UiPrimarySelection => {
                Style::new().background_color(self.ui.primary_selection_background)
            }
            StyleKey::UiPrimarySelectionAnchors => {
                Style::new().background_color(self.ui.primary_selection_anchor_background)
            }
            StyleKey::UiSecondarySelection => {
                Style::new().background_color(self.ui.secondary_selection_background)
            }
            StyleKey::UiSecondarySelectionAnchors => {
                Style::new().background_color(self.ui.secondary_selection_anchor_background)
            }
            StyleKey::UiPossibleSelection => {
                Style::new().background_color(self.ui.possible_selection_background)
            }
            StyleKey::DiagnosticsHint => self.diagnostic.hint,
            StyleKey::DiagnosticsError => self.diagnostic.error,
            StyleKey::DiagnosticsWarning => self.diagnostic.warning,
            StyleKey::DiagnosticsInformation => self.diagnostic.information,
            StyleKey::DiagnosticsDefault => self.diagnostic.default,
            StyleKey::SyntaxKeyword => self.syntax.keyword,
            StyleKey::SyntaxFunction => self.syntax.function,
            StyleKey::SyntaxComment => self.syntax.comment,
            StyleKey::SyntaxString => self.syntax.string,
            StyleKey::SyntaxDefault => self.syntax.default,
            StyleKey::SyntaxType => self.syntax.type_,
            StyleKey::HunkOld => Style::new().background_color(self.hunk_old_background),
            StyleKey::HunkNew => Style::new().background_color(self.hunk_new_background),
            StyleKey::HunkOldEmphasized => {
                Style::new().background_color(self.hunk_old_emphasized_background)
            }
            StyleKey::HunkNewEmphasized => {
                Style::new().background_color(self.hunk_new_emphasized_background)
            }
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        VSCODE_LIGHT
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiagnosticStyles {
    pub error: Style,
    pub warning: Style,
    pub information: Style,
    pub hint: Style,
    pub default: Style,
}

#[derive(Default, Clone, PartialEq, Eq, Debug)]
pub struct UiStyles {
    pub global_title: Style,
    pub window_title: Style,
    pub parent_lines_background: Color,
    pub jump_mark_odd: Style,
    pub jump_mark_even: Style,
    pub text: Style,
    pub primary_selection_background: Color,
    pub primary_selection_anchor_background: Color,
    pub primary_selection_secondary_cursor: Style,
    pub secondary_selection_background: Color,
    pub secondary_selection_anchor_background: Color,
    pub possible_selection_background: Color,
    pub secondary_selection_primary_cursor: Style,
    pub secondary_selection_secondary_cursor: Style,
    pub line_number: Style,
    pub line_number_separator: Style,
    pub bookmark: Style,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SyntaxStyles {
    pub function: Style,
    pub keyword: Style,
    pub string: Style,
    pub type_: Style,
    pub comment: Style,
    pub default: Style,
}

pub const HIGHLIGHT_NAMES: &[(&str, StyleKey)] = &[
    ("comment", StyleKey::SyntaxComment),
    ("keyword", StyleKey::SyntaxKeyword),
    ("string", StyleKey::SyntaxString),
    ("type", StyleKey::SyntaxType),
    ("tag", StyleKey::SyntaxType),
    ("function", StyleKey::SyntaxFunction),
    ("function.method", StyleKey::SyntaxFunction),
    ("function.call", StyleKey::SyntaxFunction),
    ("function.method.call", StyleKey::SyntaxFunction),
];

/// This should be constructed using the `color!` macro.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    r: u8,
    g: u8,
    b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl From<Color> for crossterm::style::Color {
    fn from(val: Color) -> Self {
        crossterm::style::Color::Rgb {
            r: val.r,
            g: val.g,
            b: val.b,
        }
    }
}
