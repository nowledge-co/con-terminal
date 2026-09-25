use con_core::config::{MAX_UI_FONT_SIZE, MIN_UI_FONT_SIZE, sanitize_terminal_font_family};
use con_terminal::{Color, TerminalTheme};
use gpui::App;
use gpui_component::highlighter::LanguageRegistry;
use gpui_component::scroll::ScrollbarMode;
use gpui_component::{Theme, ThemeMode, ThemeRegistry};
use std::borrow::Cow;

const CON_DARK_THEME: &str = include_str!("../../../assets/themes/con-dark.json");
const CON_LIGHT_THEME: &str = include_str!("../../../assets/themes/con-light.json");
const CATPPUCCIN_THEME: &str = include_str!("../../../assets/themes/catppuccin-mocha.json");
const TOKYONIGHT_THEME: &str = include_str!("../../../assets/themes/tokyonight.json");
const CON_SHELL_HIGHLIGHTS: &str = r####"
[
  (string)
  (raw_string)
  (heredoc_body)
  (heredoc_start)
  (heredoc_end)
  (ansi_c_string)
  (word)
] @string

(variable_name) @variable

[
  "export"
  "function"
  "unset"
  "local"
  "declare"
] @keyword

[
  "case"
  "do"
  "done"
  "elif"
  "else"
  "esac"
  "fi"
  "for"
  "if"
  "in"
  "select"
  "then"
  "until"
  "while"
] @keyword

(comment) @comment

((program
  .
  (comment) @preproc)
  (#match? @preproc "^#![ \t]*/"))

(function_definition
  name: (word) @title)

(command_name
  (word) @primary)

((word) @keyword
  (#match? @keyword "^--?[[:alnum:]_-]+$"))

((word) @string
  (#match? @string "^(~|\\.|/).+"))

(command
  argument: [
    (word) @text.literal
    (_
      (word) @text.literal)
  ])

[
  (file_descriptor)
  (number)
] @number

(regex) @string.regex

[
  (command_substitution)
  (process_substitution)
  (expansion)
] @embedded

[
  "$"
  "&&"
  ">"
  "<<"
  ">>"
  ">&"
  ">&-"
  "<"
  "|"
  ":"
  "//"
  "/"
  "%"
  "%%"
  "#"
  "##"
  "="
  "=="
] @operator

(test_operator) @keyword

";" @punctuation.delimiter

[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

(simple_expansion
  "$" @punctuation.special)

(expansion
  "${" @punctuation.special
  "}" @punctuation.special) @embedded

(command_substitution
  "$(" @punctuation.special
  ")" @punctuation.special)

((command
  (_) @operator)
  (#match? @operator "^-"))

(case_item
  value: (_) @string.regex)

(special_variable_name) @variable.special
"####;

// Embed IoskeleyMono font files at compile time.
const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/IoskeleyMono-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../../assets/fonts/IoskeleyMono-Bold.ttf");
const FONT_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/IoskeleyMono-Italic.ttf");
const FONT_BOLD_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/IoskeleyMono-BoldItalic.ttf");
const FONT_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/IoskeleyMono-Medium.ttf");
const FONT_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/IoskeleyMono-SemiBold.ttf");

/// Initialize the con theme system.
///
/// Registers IoskeleyMono fonts, loads built-in themes, and activates the
/// mode matching the terminal theme.
pub fn init_theme(
    cx: &mut App,
    terminal_theme: &str,
    terminal_font_family: &str,
    ui_font_family: &str,
    ui_font_size: f32,
) {
    register_command_prompt_language();
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(FONT_REGULAR),
            Cow::Borrowed(FONT_BOLD),
            Cow::Borrowed(FONT_ITALIC),
            Cow::Borrowed(FONT_BOLD_ITALIC),
            Cow::Borrowed(FONT_MEDIUM),
            Cow::Borrowed(FONT_SEMIBOLD),
        ])
        .expect("Failed to register IoskeleyMono fonts");

    for theme_json in [
        CON_DARK_THEME,
        CON_LIGHT_THEME,
        CATPPUCCIN_THEME,
        TOKYONIGHT_THEME,
    ] {
        ThemeRegistry::global_mut(cx)
            .load_themes_from_str(theme_json)
            .expect("Failed to load theme");
    }

    // For init, generate from the resolved terminal theme
    if let Some(tt) = TerminalTheme::by_name(terminal_theme) {
        apply_dynamic_theme(&tt, cx);
    } else {
        apply_gpui_theme_by_name(terminal_theme, cx);
    }
    let mode = TerminalTheme::by_name(terminal_theme)
        .map(|theme| theme_mode(&theme))
        .unwrap_or(ThemeMode::Dark);
    Theme::change(mode, None, cx);
    apply_font_overrides(terminal_font_family, ui_font_family, ui_font_size, cx);
    apply_scrollbar_overrides(cx);
}

fn register_command_prompt_language() {
    let registry = LanguageRegistry::singleton();
    if registry.language("con-shell").is_some() {
        return;
    }

    let Some(mut bash) = registry.language("bash") else {
        return;
    };

    bash.name = "con-shell".into();
    bash.highlights = CON_SHELL_HIGHLIGHTS.into();
    registry.register("con-shell", &bash);
}

/// Switch the GPUI theme to match a terminal theme.
/// Generates a dynamic GPUI theme from the terminal theme's colors.
pub fn sync_gpui_theme(
    terminal_theme: &TerminalTheme,
    terminal_font_family: &str,
    ui_font_family: &str,
    ui_font_size: f32,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    apply_dynamic_theme(terminal_theme, cx);
    let mode = theme_mode(terminal_theme);
    Theme::change(mode, Some(window), cx);
    apply_font_overrides(terminal_font_family, ui_font_family, ui_font_size, cx);
    apply_scrollbar_overrides(cx);
}

fn apply_font_overrides(
    terminal_font_family: &str,
    ui_font_family: &str,
    ui_font_size: f32,
    cx: &mut App,
) {
    Theme::global_mut(cx).mono_font_family =
        canonical_terminal_font_family(terminal_font_family).into();
    Theme::global_mut(cx).font_family = ui_font_family.to_string().into();
    let clamped_ui_font_size = ui_font_size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE);
    Theme::global_mut(cx).font_size = gpui::px(clamped_ui_font_size);
    Theme::global_mut(cx).mono_font_size = gpui::px(
        (clamped_ui_font_size - 3.0).clamp(MIN_UI_FONT_SIZE - 1.0, MAX_UI_FONT_SIZE - 3.0),
    );
}

/// Map the user-facing display name (`"Ioskeley Mono"` — what the
/// settings UI shows and what `con-core::config::default_font_family`
/// returns) to the actual `name` table entry on the registered TTFs
/// (`"IoskeleyMono"`, no space).
///
/// GPUI resolves the family string against its registered fonts. Keep
/// the user-facing settings label (`"Ioskeley Mono"`) out of GPUI's
/// hot render path and use the actual TTF family (`"IoskeleyMono"`)
/// for terminal chrome, markdown code blocks, and table text. This is
/// required on Linux's CosmicText backend and also avoids platform-
/// specific fallback behavior in StyledText code runs.
pub fn canonical_terminal_font_family(name: &str) -> String {
    let name = sanitize_terminal_font_family(name);
    // Normalize aggressively: trim, lowercase, strip whitespace and
    // hyphens. That way `"Ioskeley Mono"`, `"IoskeleyMono"`,
    // `"Ioskeley-Mono"`, `"ioskeley mono"`, `" IOSKELEY  MONO "`,
    // and any other casing / spacing the user might paste into the
    // config or settings UI all resolve to the registered TTF family.
    let key: String = name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();
    if key == "ioskeleymono" {
        return "IoskeleyMono".to_string();
    }

    name.trim().to_string()
}

/// Apply con's scrollbar overrides after any Theme::change call.
/// Must run AFTER Theme::change because it resets colors from the theme config.
fn apply_scrollbar_overrides(cx: &mut App) {
    Theme::global_mut(cx).scrollbar_mode = ScrollbarMode::Hover;
    Theme::global_mut(cx).colors.scrollbar = gpui::transparent_black();
    // Base primitives and TextView cache a projection of the styled theme.
    // Publish both these overrides and the preceding font overrides together.
    Theme::sync_base(cx);
}

/// Generate a GPUI theme dynamically from terminal ANSI colors and register it.
fn apply_dynamic_theme(tt: &TerminalTheme, cx: &mut App) {
    let json = generate_gpui_theme_json(tt);
    // Register (or re-register) the dynamic theme
    // ThemeRegistry skips duplicates, so we directly set the theme config
    match serde_json::from_str::<gpui_component::ThemeSet>(&json) {
        Ok(theme_set) => {
            for theme_config in theme_set.themes {
                let rc = std::rc::Rc::new(theme_config);
                if tt.is_dark() {
                    Theme::global_mut(cx).dark_theme = rc;
                } else {
                    Theme::global_mut(cx).light_theme = rc;
                }
            }
        }
        Err(e) => {
            log::error!("Failed to parse generated theme JSON: {e}");
            apply_gpui_theme_by_name(&tt.name, cx);
        }
    }
}

/// Fallback: map terminal theme name to a pre-registered GPUI theme.
fn apply_gpui_theme_by_name(terminal_theme_name: &str, cx: &mut App) {
    let (dark_name, light_name) = match terminal_theme_name {
        "catppuccin-mocha" => ("Catppuccin Mocha", "Con Light"),
        "tokyonight" => ("Tokyo Night", "Con Light"),
        _ => ("Con Dark", "Con Light"),
    };
    if let Some(d) = ThemeRegistry::global(cx).themes().get(dark_name).cloned() {
        Theme::global_mut(cx).dark_theme = d;
    }
    if let Some(l) = ThemeRegistry::global(cx).themes().get(light_name).cloned() {
        Theme::global_mut(cx).light_theme = l;
    }
}

/// Generate a complete GPUI theme JSON string from terminal theme colors.
///
/// Maps terminal ANSI palette to GPUI semantic colors:
/// - primary = blue (ansi[4]) — the UI accent color
/// - danger = red (ansi[1])
/// - success = green (ansi[2])
/// - warning = yellow (ansi[3])
/// - info = cyan (ansi[6])
/// - Surface colors derived from bg/fg with blending
fn generate_gpui_theme_json(tt: &TerminalTheme) -> String {
    let bg = tt.background;
    let fg = tt.foreground;
    let red = tt.ansi[1];
    let green = tt.ansi[2];
    let yellow = tt.ansi[3];
    let blue = tt.ansi[4];
    let magenta = tt.ansi[5];
    let cyan = tt.ansi[6];

    let is_dark = tt.is_dark();
    let mode = if is_dark { "dark" } else { "light" };

    // Generate surface colors by blending bg toward fg
    let surface1 = blend(bg, fg, 0.06);
    let surface2 = blend(bg, fg, 0.12);
    let surface3 = blend(bg, fg, 0.18);
    let fg = contrasting_text(fg, &[bg]);
    let muted_fg = contrasting_text(blend(fg, bg, 0.45), &[surface1]);

    // Primary contrast — use bg as text on primary buttons for max contrast
    let primary_hover = if is_dark {
        darken(blue, 0.15)
    } else {
        darken(blue, 0.12)
    };
    let primary_active = darken(blue, 0.25);
    let primary_fg = contrasting_text(bg, &[blue, primary_hover, primary_active]);

    // Danger hover/active
    let danger_hover = if is_dark {
        lighten(red, 0.1)
    } else {
        darken(red, 0.1)
    };
    let danger_active = darken(red, 0.2);
    let secondary_fg = contrasting_text(fg, &[surface1, surface2, surface3]);
    let accent_fg = contrasting_text(fg, &[surface3]);
    let success_fg = contrasting_text(fg, &[green]);
    let danger_fg = contrasting_text(fg, &[red, danger_hover, danger_active]);
    let warning_fg = contrasting_text(fg, &[yellow]);
    let info_fg = contrasting_text(fg, &[cyan]);

    let theme_name = format!("con-gen-{}", tt.name);

    format!(
        r#"{{
  "name": "{theme_name}",
  "author": "con (generated)",
  "themes": [
    {{
      "name": "{theme_name}",
      "mode": "{mode}",
      "is_default": false,
      "font.size": 14,
      "font.family": ".SystemUIFont",
      "mono_font.family": "Ioskeley Mono",
      "mono_font.size": 14,
      "radius": 8,
      "radius_lg": 12,
      "shadow": false,
      "colors": {{
        "background": "{bg_hex}",
        "foreground": "{fg_hex}",
        "border": "{border}",
        "input.border": "{border}",
        "caret": "{blue_hex}",
        "ring": "{blue_hex}",

        "primary.background": "{blue_hex}",
        "primary.foreground": "{primary_fg_hex}",
        "primary.hover.background": "{primary_hover_hex}",
        "primary.active.background": "{primary_active_hex}",

        "secondary.background": "{surface1_hex}",
        "secondary.foreground": "{secondary_fg_hex}",
        "secondary.hover.background": "{surface2_hex}",
        "secondary.active.background": "{surface3_hex}",

        "muted.background": "{surface1_hex}",
        "muted.foreground": "{muted_fg_hex}",

        "accent.background": "{surface3_hex}",
        "accent.foreground": "{accent_fg_hex}",

        "success.background": "{green_hex}",
        "success.foreground": "{success_fg_hex}",

        "danger.background": "{red_hex}",
        "danger.foreground": "{danger_fg_hex}",
        "danger.hover.background": "{danger_hover_hex}",
        "danger.active.background": "{danger_active_hex}",

        "warning.background": "{yellow_hex}",
        "warning.foreground": "{warning_fg_hex}",

        "info.background": "{cyan_hex}",
        "info.foreground": "{info_fg_hex}",

        "title_bar.background": "{surface1_hex}",
        "title_bar.border": "{border}",

        "sidebar.background": "{surface1_hex}",
        "sidebar.foreground": "{secondary_fg_hex}",
        "sidebar.border": "{border}",

        "list.active.background": "{list_active}",

        "selection.background": "{selection}",

        "scrollbar.thumb.background": "{scrollbar}",
        "scrollbar.thumb.hover.background": "{surface3_hex}",

        "base.red": "{red_hex}",
        "base.orange": "{orange_hex}",
        "base.yellow": "{yellow_hex}",
        "base.green": "{green_hex}",
        "base.cyan": "{cyan_hex}",
        "base.blue": "{blue_hex}",
        "base.purple": "{purple_hex}",
        "base.magenta": "{magenta_hex}"
      }},
      "highlight": {{
        "editor.foreground": "{fg_hex}",
        "editor.background": "{bg_hex}",
        "editor.active_line.background": "{surface1_hex}",
        "editor.line_number": "{muted_fg_hex}",
        "editor.active_line_number": "{blue_hex}",
        "editor.invisible": "{muted_invisible}",
        "conflict": "{yellow_hex}",
        "created": "{green_hex}",
        "hidden": "{muted_fg_hex}",
        "hint": "{muted_fg_hex}",
        "modified": "{orange_hex}",
        "predictive": "{muted_fg_hex}",
        "warning": "{yellow_hex}",
        "syntax": {{
          "attribute": {{ "color": "{blue_hex}" }},
          "boolean": {{ "color": "{yellow_hex}" }},
          "comment": {{ "color": "{muted_fg_hex}" }},
          "comment.doc": {{ "color": "{muted_fg_hex}" }},
          "constant": {{ "color": "{orange_hex}" }},
          "constructor": {{ "color": "{blue_hex}" }},
          "emphasis": {{ "color": "{cyan_hex}", "font_style": "italic" }},
          "emphasis.strong": {{ "color": "{cyan_hex}", "font_weight": 700 }},
          "enum": {{ "color": "{yellow_hex}" }},
          "function": {{ "color": "{orange_hex}" }},
          "hint": {{ "color": "{muted_fg_hex}" }},
          "keyword": {{ "color": "{green_hex}" }},
          "label": {{ "color": "{blue_hex}" }},
          "link_text": {{ "color": "{cyan_hex}" }},
          "link_uri": {{ "color": "{cyan_hex}" }},
          "number": {{ "color": "{purple_hex}" }},
          "operator": {{ "color": "{muted_fg_hex}" }},
          "predictive": {{ "color": "{muted_fg_hex}" }},
          "preproc": {{ "color": "{magenta_hex}" }},
          "primary": {{ "color": "{cyan_hex}" }},
          "property": {{ "color": "{orange_hex}" }},
          "punctuation": {{ "color": "{muted_fg_hex}" }},
          "punctuation.bracket": {{ "color": "{muted_fg_hex}" }},
          "punctuation.delimiter": {{ "color": "{muted_fg_hex}" }},
          "string": {{ "color": "{cyan_hex}" }},
          "string.escape": {{ "color": "{cyan_hex}" }},
          "string.regex": {{ "color": "{cyan_hex}" }},
          "string.special": {{ "color": "{cyan_hex}" }},
          "tag": {{ "color": "{blue_hex}" }},
          "text.literal": {{ "color": "{cyan_hex}" }},
          "title": {{ "color": "{yellow_hex}" }},
          "type": {{ "color": "{yellow_hex}" }},
          "variable": {{ "color": "{blue_hex}" }},
          "variable.special": {{ "color": "{blue_hex}" }},
          "variant": {{ "color": "{cyan_hex}" }}
        }}
      }}
    }}
  ]
}}"#,
        bg_hex = hex(bg),
        fg_hex = hex(fg),
        border = hex(surface2),
        primary_fg_hex = hex(primary_fg),
        secondary_fg_hex = hex(secondary_fg),
        accent_fg_hex = hex(accent_fg),
        success_fg_hex = hex(success_fg),
        danger_fg_hex = hex(danger_fg),
        warning_fg_hex = hex(warning_fg),
        info_fg_hex = hex(info_fg),
        primary_hover_hex = hex(primary_hover),
        primary_active_hex = hex(primary_active),
        surface1_hex = hex(surface1),
        surface2_hex = hex(surface2),
        surface3_hex = hex(surface3),
        muted_fg_hex = hex(muted_fg),
        red_hex = hex(red),
        green_hex = hex(green),
        yellow_hex = hex(yellow),
        blue_hex = hex(blue),
        cyan_hex = hex(cyan),
        magenta_hex = hex(magenta),
        danger_hover_hex = hex(danger_hover),
        danger_active_hex = hex(danger_active),
        orange_hex = hex_rgb(
            tt.ansi[9].r.max(tt.ansi[3].r),
            tt.ansi[9].g.min(tt.ansi[3].g),
            tt.ansi[9].b.min(tt.ansi[3].b)
        ),
        purple_hex = hex(tt.ansi[13]),
        list_active = hex_alpha(blue, 0x18),
        selection = hex_alpha(blue, 0x28),
        scrollbar = hex_alpha(surface2, 0x80),
        muted_invisible = hex_alpha(muted_fg, 0x66),
    )
}

// ── Color helpers ──────────────────────────────────────────────

fn hex(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

fn hex_alpha(c: Color, alpha: u8) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, alpha)
}

#[allow(clippy::many_single_char_names)]
fn hex_rgb(r: u8, g: u8, b: u8) -> String {
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

fn theme_mode(theme: &TerminalTheme) -> ThemeMode {
    if theme.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

fn contrast_ratio(a: Color, b: Color) -> f64 {
    let lighter = a.relative_luminance().max(b.relative_luminance());
    let darker = a.relative_luminance().min(b.relative_luminance());
    (lighter + 0.05) / (darker + 0.05)
}

fn contrasting_text(preferred: Color, backgrounds: &[Color]) -> Color {
    const MIN_CONTRAST: f64 = 4.5;
    if backgrounds
        .iter()
        .all(|background| contrast_ratio(preferred, *background) >= MIN_CONTRAST)
    {
        return preferred;
    }

    let black = Color::rgb(0, 0, 0);
    let white = Color::rgb(255, 255, 255);
    let minimum_contrast = |candidate| {
        backgrounds
            .iter()
            .map(|background| contrast_ratio(candidate, *background))
            .fold(f64::INFINITY, f64::min)
    };
    let target = if minimum_contrast(black) >= minimum_contrast(white) {
        black
    } else {
        white
    };
    // Preserve the preferred tone instead of turning every low-contrast
    // secondary label or placeholder into the strongest possible text.
    for step in 1..=255 {
        let candidate = blend(preferred, target, f64::from(step) / 255.0);
        if minimum_contrast(candidate) >= MIN_CONTRAST {
            return candidate;
        }
    }
    target
}

fn blend(base: Color, target: Color, amount: f64) -> Color {
    let r = (base.r as f64 + (target.r as f64 - base.r as f64) * amount) as u8;
    let g = (base.g as f64 + (target.g as f64 - base.g as f64) * amount) as u8;
    let b = (base.b as f64 + (target.b as f64 - base.b as f64) * amount) as u8;
    Color::rgb(r, g, b)
}

fn darken(c: Color, amount: f64) -> Color {
    let factor = 1.0 - amount;
    Color::rgb(
        (c.r as f64 * factor) as u8,
        (c.g as f64 * factor) as u8,
        (c.b as f64 * factor) as u8,
    )
}

fn lighten(c: Color, amount: f64) -> Color {
    Color::rgb(
        (c.r as f64 + (255.0 - c.r as f64) * amount) as u8,
        (c.g as f64 + (255.0 - c.g as f64) * amount) as u8,
        (c.b as f64 + (255.0 - c.b as f64) * amount) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn overrides_reach_base_theme_after_each_mode_change(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        cx.update(|cx| {
            for mode in [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Light] {
                Theme::set_scrollbar_mode(ScrollbarMode::Scrolling, cx);
                Theme::change(mode, None, cx);
                apply_font_overrides("Ioskeley Mono", ".SystemUIFont", 19.0, cx);
                apply_scrollbar_overrides(cx);

                let base = gpui_base::Theme::global(cx);
                assert_eq!(base.scrollbar.mode(), ScrollbarMode::Hover);
                assert_eq!(base.tokens, Theme::global(cx).semantic_tokens());
            }
        });
    }

    fn generated_colors(theme: &TerminalTheme) -> serde_json::Map<String, serde_json::Value> {
        let json: serde_json::Value =
            serde_json::from_str(&generate_gpui_theme_json(theme)).unwrap();
        json["themes"][0]["colors"].as_object().unwrap().clone()
    }

    fn color(value: &serde_json::Value) -> Color {
        let hex = value.as_str().unwrap().trim_start_matches('#');
        Color::rgb(
            u8::from_str_radix(&hex[0..2], 16).unwrap(),
            u8::from_str_radix(&hex[2..4], 16).unwrap(),
            u8::from_str_radix(&hex[4..6], 16).unwrap(),
        )
    }

    #[test]
    fn flexoki_muted_text_stays_readable_without_outshining_body_text() {
        for theme in [
            TerminalTheme::flexoki_dark(),
            TerminalTheme::flexoki_light(),
        ] {
            let colors = generated_colors(&theme);
            let background = color(&colors["muted.background"]);
            let muted = color(&colors["muted.foreground"]);
            let foreground = color(&colors["foreground"]);

            assert!(contrast_ratio(muted, background) >= 4.5);
            assert!(
                contrast_ratio(muted, background) < contrast_ratio(foreground, background),
                "{}: muted text must remain less prominent than body text",
                theme.name,
            );
        }
    }

    #[test]
    fn contrast_correction_preserves_valid_colors_and_checks_every_background() {
        let preferred = Color::rgb(0x78, 0x77, 0x72);
        let black = Color::rgb(0, 0, 0);
        let surface = Color::rgb(0x30, 0x28, 0x20);
        assert_eq!(contrasting_text(preferred, &[black]), preferred);

        let corrected = contrasting_text(preferred, &[black, surface]);
        assert_ne!(corrected, preferred);
        assert_ne!(corrected, Color::rgb(255, 255, 255));
        for background in [black, surface] {
            assert!(contrast_ratio(corrected, background) >= 4.5);
        }
    }

    #[test]
    fn low_contrast_normal_text_is_corrected() {
        let mut theme = TerminalTheme::flexoki_light();
        theme.foreground = Color::rgb(0xDD, 0xDD, 0xDD);
        let colors = generated_colors(&theme);

        assert!(contrast_ratio(color(&colors["foreground"]), theme.background) >= 4.5);
        assert!(
            contrast_ratio(
                color(&colors["muted.foreground"]),
                color(&colors["muted.background"])
            ) >= 4.5
        );
    }

    #[test]
    fn pale_yellow_warning_gets_its_own_contrasting_text() {
        let mut theme = TerminalTheme::tokyonight();
        theme.ansi[3] = Color::rgb(0xFF, 0xF4, 0xA3);
        let colors = generated_colors(&theme);

        assert!(contrast_ratio(color(&colors["warning.foreground"]), theme.ansi[3]) >= 4.5);
    }

    #[test]
    fn tinted_dark_background_selects_dark_mode_without_touching_palette() {
        let mut theme = TerminalTheme::flexoki_light();
        theme.name = "misleading-light".into();
        theme.background = Color::rgb(0x12, 0x28, 0x35);
        let palette = theme.ansi;
        let json: serde_json::Value =
            serde_json::from_str(&generate_gpui_theme_json(&theme)).unwrap();

        assert_eq!(json["themes"][0]["mode"], "dark");
        assert_eq!(theme.ansi, palette);
    }
}
