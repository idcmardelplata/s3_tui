use ratatui::style::{Color, Modifier, Style};

/// Centralised visual theme for the whole application.
///
/// Keeping every colour and modifier in one place makes the UI coherent and
/// easy to restyle without hunting for magic values.
pub struct Theme {
    /// Colour used for the outer borders of focused, active panels.
    pub accent: Color,
    /// Colour used for borders of secondary / inactive panels.
    pub muted_border: Color,
    /// Highlight colour for the currently selected row.
    pub highlight: Color,
    /// Background colour for the currently selected row.
    pub selected_bg: Color,
    /// Foreground colour for folder names.
    pub folder: Color,
    /// Foreground for primary text / values.
    pub text: Color,
    /// Foreground for secondary / de-emphasised text.
    pub muted: Color,
    /// Foreground for success messages.
    pub success: Color,
    /// Foreground for error / destructive messages.
    pub error: Color,
    /// Foreground for warnings.
    pub warning: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            accent: Color::Cyan,
            muted_border: Color::DarkGray,
            highlight: Color::Yellow,
            selected_bg: Color::Blue,
            folder: Color::LightBlue,
            text: Color::White,
            muted: Color::Gray,
            success: Color::Green,
            error: Color::Red,
            warning: Color::Yellow,
        }
    }

    /// Border style for the active panel (the one being interacted with).
    pub fn active_border(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// Border style for an inactive panel.
    pub fn inactive_border(&self) -> Style {
        Style::default().fg(self.muted_border)
    }

    /// Style for an emphasised line: the title bar / vertical bar of a panel.
    pub fn panel_title(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// Style applied to the selected row of a list or table.
    pub fn selected_row(&self) -> Style {
        Style::default()
            .bg(self.selected_bg)
            .add_modifier(Modifier::BOLD)
    }
}

/// Activate a border for the panel that currently holds the cursor.
fn border_style(theme: &Theme, active: bool) -> Style {
    if active {
        theme.active_border()
    } else {
        theme.inactive_border()
    }
}

/// Returns the border style to use for a panel. `active` is `true` when the
/// panel holds the cursor for that navigation mode.
pub fn panel_border(theme: &Theme, active: bool) -> Style {
    border_style(theme, active)
}
