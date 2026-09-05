use ratatui::style::{Color, Modifier, Style};
use ratatui_themekit::{BUILTIN_THEMES, NO_COLOR, ThemeData, ThemeExt, no_color_active};

pub type Theme = ThemeData;

/// Theme ID used when nothing else is configured.
pub const DEFAULT_THEME_ID: &str = "catppuccin";

#[must_use]
pub fn resolve() -> Theme {
    if no_color_active() {
        return NO_COLOR;
    }
    let id = effective_theme_id();
    from_id(&id)
}

fn effective_theme_id() -> String {
    std::env::var("S3TUI_THEME")
        .ok()
        .or_else(|| crate::config::load().ok().and_then(|c| c.effective_theme()))
        .unwrap_or_else(|| DEFAULT_THEME_ID.to_string())
}

#[must_use]
pub fn from_id(id: &str) -> Theme {
    BUILTIN_THEMES
        .iter()
        .find(|t| t.id == id)
        .copied()
        .unwrap_or_else(|| {
            eprintln!(
                "warning: unknown theme '{id}', using '{}'",
                DEFAULT_THEME_ID
            );
            ratatui_themekit::default_theme()
        })
}

#[must_use]
pub fn active_border(theme: &Theme) -> Style {
    theme.style_accent().add_modifier(Modifier::BOLD)
}

#[must_use]
pub fn inactive_border(theme: &Theme) -> Style {
    theme.style_border()
}

#[must_use]
pub fn panel_border(theme: &Theme, active: bool) -> Style {
    if active {
        active_border(theme)
    } else {
        inactive_border(theme)
    }
}

#[must_use]
pub fn panel_title(theme: &Theme) -> Style {
    theme.style_accent().add_modifier(Modifier::BOLD)
}

#[must_use]
pub fn selected_row(theme: &Theme) -> Style {
    theme.style_surface().add_modifier(Modifier::BOLD)
}

#[must_use]
pub fn folder_color(theme: &Theme) -> Color {
    theme.info
}

#[must_use]
pub fn base(theme: &Theme) -> Style {
    theme.style_base()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialises tests that mutate the process environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set_env(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) }
    }

    fn unset_env(key: &str) {
        unsafe { std::env::remove_var(key) }
    }

    #[test]
    fn from_id_resolves_builtin() {
        assert_eq!(from_id("nord").id, "nord");
        assert_eq!(from_id("tokyo-night").id, "tokyo-night");
    }

    #[test]
    fn from_id_falls_back_on_unknown() {
        assert_eq!(from_id("not-a-theme").id, DEFAULT_THEME_ID);
    }

    #[test]
    fn resolve_honours_s3tui_theme_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        set_env("S3TUI_THEME", "dracula");
        assert_eq!(resolve().id, "dracula");
        unset_env("S3TUI_THEME");
    }

    #[test]
    fn no_color_overrides_explicit_theme() {
        let _guard = ENV_LOCK.lock().unwrap();
        set_env("NO_COLOR", "1");
        set_env("S3TUI_THEME", "dracula");
        assert_eq!(resolve(), NO_COLOR);
        assert_eq!(resolve().accent, Color::Reset);
        unset_env("NO_COLOR");
        unset_env("S3TUI_THEME");
    }

    #[test]
    fn unknown_env_theme_falls_back() {
        let _guard = ENV_LOCK.lock().unwrap();
        set_env("S3TUI_THEME", "fancy");
        assert_eq!(resolve().id, DEFAULT_THEME_ID);
        unset_env("S3TUI_THEME");
    }

    #[test]
    fn style_helpers_use_theme_slots() {
        let theme = BUILTIN_THEMES[0];
        assert_eq!(active_border(&theme).fg, Some(theme.accent));
        assert!(active_border(&theme).add_modifier.contains(Modifier::BOLD));
        assert_eq!(inactive_border(&theme).fg, Some(theme.border));
        assert_eq!(panel_border(&theme, true), active_border(&theme));
        assert_eq!(panel_border(&theme, false), inactive_border(&theme));
        assert_eq!(
            selected_row(&theme).bg,
            Some(theme.surface),
            "selected row uses surface background"
        );
        assert_eq!(folder_color(&theme), theme.info);
        assert_eq!(base(&theme).bg, Some(theme.background));
    }
}
