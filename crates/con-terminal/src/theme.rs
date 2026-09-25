/// RGBA color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// WCAG relative luminance in the sRGB color space.
    pub fn relative_luminance(self) -> f64 {
        fn linear(channel: u8) -> f64 {
            let value = channel as f64 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }

        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }
}

/// Terminal color theme used for con chrome and Ghostty palette generation.
#[derive(Debug, Clone)]
pub struct TerminalTheme {
    pub name: String,
    pub foreground: Color,
    pub background: Color,
    pub ansi: [Color; 16],
}

impl TerminalTheme {
    /// Compatibility names authored by older Con versions, not Ghostty's
    /// case-sensitive native theme catalog (for example `Dracula`).
    pub fn legacy_builtin(name: &str) -> Option<Self> {
        if name != name.to_lowercase() {
            return None;
        }
        Self::by_name(name).filter(|theme| Self::available().contains(&theme.name.as_str()))
    }

    /// Whether the background should use dark-mode UI chrome.
    pub fn is_dark(&self) -> bool {
        self.background.relative_luminance() < 0.5
    }

    /// Serialize this theme as a standalone Ghostty theme resource.
    pub fn to_ghostty_format(&self) -> String {
        let mut output = format!(
            "foreground = #{:02x}{:02x}{:02x}\nbackground = #{:02x}{:02x}{:02x}\n",
            self.foreground.r,
            self.foreground.g,
            self.foreground.b,
            self.background.r,
            self.background.g,
            self.background.b
        );
        for (index, color) in self.ansi.iter().enumerate() {
            output.push_str(&format!(
                "palette = {index}=#{:02x}{:02x}{:02x}\n",
                color.r, color.g, color.b
            ));
        }
        output
    }

    pub fn flexoki_dark() -> Self {
        Self {
            name: "flexoki-dark".into(),
            foreground: Color::rgb(0xCE, 0xCD, 0xC3),
            background: Color::rgb(0x10, 0x0F, 0x0F),
            ansi: [
                Color::rgb(0x10, 0x0F, 0x0F),
                Color::rgb(0xD1, 0x4D, 0x41),
                Color::rgb(0x87, 0x9A, 0x39),
                Color::rgb(0xD0, 0xA2, 0x15),
                Color::rgb(0x43, 0x85, 0xBE),
                Color::rgb(0x8B, 0x7E, 0xC8),
                Color::rgb(0x3A, 0xA9, 0x9F),
                Color::rgb(0xCE, 0xCD, 0xC3),
                Color::rgb(0x57, 0x56, 0x53),
                Color::rgb(0xD1, 0x4D, 0x41),
                Color::rgb(0x87, 0x9A, 0x39),
                Color::rgb(0xD0, 0xA2, 0x15),
                Color::rgb(0x43, 0x85, 0xBE),
                Color::rgb(0xCE, 0x5D, 0x97),
                Color::rgb(0x3A, 0xA9, 0x9F),
                Color::rgb(0xCE, 0xCD, 0xC3),
            ],
        }
    }

    pub fn flexoki_light() -> Self {
        Self {
            name: "flexoki-light".into(),
            foreground: Color::rgb(0x10, 0x0F, 0x0F),
            background: Color::rgb(0xFF, 0xFC, 0xF0),
            ansi: [
                Color::rgb(0x10, 0x0F, 0x0F),
                Color::rgb(0xAF, 0x30, 0x29),
                Color::rgb(0x66, 0x80, 0x0B),
                Color::rgb(0xAD, 0x8A, 0x01),
                Color::rgb(0x20, 0x5E, 0xA6),
                Color::rgb(0x5E, 0x40, 0x9D),
                Color::rgb(0x24, 0x83, 0x7B),
                Color::rgb(0xCE, 0xCD, 0xC3),
                Color::rgb(0x87, 0x85, 0x80),
                Color::rgb(0xD1, 0x4D, 0x41),
                Color::rgb(0x87, 0x9A, 0x39),
                Color::rgb(0xD0, 0xA2, 0x15),
                Color::rgb(0x43, 0x85, 0xBE),
                Color::rgb(0xCE, 0x5D, 0x97),
                Color::rgb(0x3A, 0xA9, 0x9F),
                Color::rgb(0xFF, 0xFC, 0xF0),
            ],
        }
    }

    pub fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin-mocha".into(),
            foreground: Color::rgb(0xCD, 0xD6, 0xF4),
            background: Color::rgb(0x1E, 0x1E, 0x2E),
            ansi: [
                Color::rgb(0x45, 0x47, 0x5A),
                Color::rgb(0xF3, 0x8B, 0xA8),
                Color::rgb(0xA6, 0xE3, 0xA1),
                Color::rgb(0xF9, 0xE2, 0xAF),
                Color::rgb(0x89, 0xB4, 0xFA),
                Color::rgb(0xCB, 0xA6, 0xF7),
                Color::rgb(0x94, 0xE2, 0xD5),
                Color::rgb(0xBA, 0xC2, 0xDE),
                Color::rgb(0x58, 0x5B, 0x70),
                Color::rgb(0xF3, 0x8B, 0xA8),
                Color::rgb(0xA6, 0xE3, 0xA1),
                Color::rgb(0xF9, 0xE2, 0xAF),
                Color::rgb(0x89, 0xB4, 0xFA),
                Color::rgb(0xCB, 0xA6, 0xF7),
                Color::rgb(0x94, 0xE2, 0xD5),
                Color::rgb(0xCD, 0xD6, 0xF4),
            ],
        }
    }

    pub fn tokyonight() -> Self {
        Self {
            name: "tokyonight".into(),
            foreground: Color::rgb(0xC0, 0xCA, 0xF5),
            background: Color::rgb(0x1A, 0x1B, 0x26),
            ansi: [
                Color::rgb(0x15, 0x16, 0x1E),
                Color::rgb(0xF7, 0x76, 0x8E),
                Color::rgb(0x9E, 0xCE, 0x6A),
                Color::rgb(0xE0, 0xAF, 0x68),
                Color::rgb(0x7A, 0xA2, 0xF7),
                Color::rgb(0xBB, 0x9A, 0xF7),
                Color::rgb(0x7D, 0xCF, 0xFF),
                Color::rgb(0xA9, 0xB1, 0xD6),
                Color::rgb(0x41, 0x48, 0x68),
                Color::rgb(0xF7, 0x76, 0x8E),
                Color::rgb(0x9E, 0xCE, 0x6A),
                Color::rgb(0xE0, 0xAF, 0x68),
                Color::rgb(0x7A, 0xA2, 0xF7),
                Color::rgb(0xBB, 0x9A, 0xF7),
                Color::rgb(0x7D, 0xCF, 0xFF),
                Color::rgb(0xC0, 0xCA, 0xF5),
            ],
        }
    }

    pub fn dracula() -> Self {
        Self {
            name: "dracula".into(),
            foreground: Color::rgb(0xF8, 0xF8, 0xF2),
            background: Color::rgb(0x28, 0x2A, 0x36),
            ansi: [
                Color::rgb(0x21, 0x22, 0x2C),
                Color::rgb(0xFF, 0x55, 0x55),
                Color::rgb(0x50, 0xFA, 0x7B),
                Color::rgb(0xF1, 0xFA, 0x8C),
                Color::rgb(0xBD, 0x93, 0xF9),
                Color::rgb(0xFF, 0x79, 0xC6),
                Color::rgb(0x8B, 0xE9, 0xFD),
                Color::rgb(0xF8, 0xF8, 0xF2),
                Color::rgb(0x62, 0x72, 0xA4),
                Color::rgb(0xFF, 0x6E, 0x6E),
                Color::rgb(0x69, 0xFF, 0x94),
                Color::rgb(0xFF, 0xFF, 0xA5),
                Color::rgb(0xD6, 0xAC, 0xFF),
                Color::rgb(0xFF, 0x92, 0xDF),
                Color::rgb(0xA4, 0xFF, 0xFF),
                Color::rgb(0xFF, 0xFF, 0xFF),
            ],
        }
    }

    pub fn nord() -> Self {
        Self {
            name: "nord".into(),
            foreground: Color::rgb(0xD8, 0xDE, 0xE9),
            background: Color::rgb(0x2E, 0x34, 0x40),
            ansi: [
                Color::rgb(0x3B, 0x42, 0x52),
                Color::rgb(0xBF, 0x61, 0x6A),
                Color::rgb(0xA3, 0xBE, 0x8C),
                Color::rgb(0xEB, 0xCB, 0x8B),
                Color::rgb(0x81, 0xA1, 0xC1),
                Color::rgb(0xB4, 0x8E, 0xAD),
                Color::rgb(0x88, 0xC0, 0xD0),
                Color::rgb(0xE5, 0xE9, 0xF0),
                Color::rgb(0x59, 0x63, 0x77),
                Color::rgb(0xBF, 0x61, 0x6A),
                Color::rgb(0xA3, 0xBE, 0x8C),
                Color::rgb(0xEB, 0xCB, 0x8B),
                Color::rgb(0x81, 0xA1, 0xC1),
                Color::rgb(0xB4, 0x8E, 0xAD),
                Color::rgb(0x8F, 0xBC, 0xBB),
                Color::rgb(0xEC, 0xEF, 0xF4),
            ],
        }
    }

    pub fn rose_pine() -> Self {
        Self {
            name: "rose-pine".into(),
            foreground: Color::rgb(0xE0, 0xDE, 0xF4),
            background: Color::rgb(0x19, 0x17, 0x24),
            ansi: [
                Color::rgb(0x26, 0x23, 0x3A),
                Color::rgb(0xEB, 0x6F, 0x92),
                Color::rgb(0x31, 0x74, 0x8F),
                Color::rgb(0xF6, 0xC1, 0x77),
                Color::rgb(0x9C, 0xCF, 0xD8),
                Color::rgb(0xC4, 0xA7, 0xE7),
                Color::rgb(0xEA, 0x9A, 0x97),
                Color::rgb(0xE0, 0xDE, 0xF4),
                Color::rgb(0x6E, 0x6A, 0x86),
                Color::rgb(0xEB, 0x6F, 0x92),
                Color::rgb(0x31, 0x74, 0x8F),
                Color::rgb(0xF6, 0xC1, 0x77),
                Color::rgb(0x9C, 0xCF, 0xD8),
                Color::rgb(0xC4, 0xA7, 0xE7),
                Color::rgb(0xEA, 0x9A, 0x97),
                Color::rgb(0xE0, 0xDE, 0xF4),
            ],
        }
    }

    pub fn gruvbox_dark() -> Self {
        Self {
            name: "gruvbox-dark".into(),
            foreground: Color::rgb(0xEB, 0xDB, 0xB2),
            background: Color::rgb(0x28, 0x28, 0x28),
            ansi: [
                Color::rgb(0x28, 0x28, 0x28),
                Color::rgb(0xCC, 0x24, 0x1D),
                Color::rgb(0x98, 0x97, 0x1A),
                Color::rgb(0xD7, 0x99, 0x21),
                Color::rgb(0x45, 0x85, 0x88),
                Color::rgb(0xB1, 0x62, 0x86),
                Color::rgb(0x68, 0x9D, 0x6A),
                Color::rgb(0xA8, 0x99, 0x84),
                Color::rgb(0x92, 0x83, 0x74),
                Color::rgb(0xFB, 0x49, 0x34),
                Color::rgb(0xB8, 0xBB, 0x26),
                Color::rgb(0xFA, 0xBD, 0x2F),
                Color::rgb(0x83, 0xA5, 0x98),
                Color::rgb(0xD3, 0x86, 0x9B),
                Color::rgb(0x8E, 0xC0, 0x7C),
                Color::rgb(0xEB, 0xDB, 0xB2),
            ],
        }
    }

    pub fn solarized_dark() -> Self {
        Self {
            name: "solarized-dark".into(),
            foreground: Color::rgb(0x83, 0x94, 0x96),
            background: Color::rgb(0x00, 0x2B, 0x36),
            ansi: [
                Color::rgb(0x07, 0x36, 0x42),
                Color::rgb(0xDC, 0x32, 0x2F),
                Color::rgb(0x85, 0x99, 0x00),
                Color::rgb(0xB5, 0x89, 0x00),
                Color::rgb(0x26, 0x8B, 0xD2),
                Color::rgb(0xD3, 0x36, 0x82),
                Color::rgb(0x2A, 0xA1, 0x98),
                Color::rgb(0xEE, 0xE8, 0xD5),
                Color::rgb(0x00, 0x2B, 0x36),
                Color::rgb(0xCB, 0x4B, 0x16),
                Color::rgb(0x58, 0x6E, 0x75),
                Color::rgb(0x65, 0x7B, 0x83),
                Color::rgb(0x83, 0x94, 0x96),
                Color::rgb(0x6C, 0x71, 0xC4),
                Color::rgb(0x93, 0xA1, 0xA1),
                Color::rgb(0xFD, 0xF6, 0xE3),
            ],
        }
    }

    pub fn one_half_dark() -> Self {
        Self {
            name: "one-half-dark".into(),
            foreground: Color::rgb(0xDC, 0xDF, 0xE4),
            background: Color::rgb(0x28, 0x2C, 0x34),
            ansi: [
                Color::rgb(0x28, 0x2C, 0x34),
                Color::rgb(0xE0, 0x6C, 0x75),
                Color::rgb(0x98, 0xC3, 0x79),
                Color::rgb(0xE5, 0xC0, 0x7B),
                Color::rgb(0x61, 0xAF, 0xEF),
                Color::rgb(0xC6, 0x78, 0xDD),
                Color::rgb(0x56, 0xB6, 0xC2),
                Color::rgb(0xDC, 0xDF, 0xE4),
                Color::rgb(0x5C, 0x63, 0x70),
                Color::rgb(0xE0, 0x6C, 0x75),
                Color::rgb(0x98, 0xC3, 0x79),
                Color::rgb(0xE5, 0xC0, 0x7B),
                Color::rgb(0x61, 0xAF, 0xEF),
                Color::rgb(0xC6, 0x78, 0xDD),
                Color::rgb(0x56, 0xB6, 0xC2),
                Color::rgb(0xDC, 0xDF, 0xE4),
            ],
        }
    }

    pub fn kanagawa_wave() -> Self {
        Self {
            name: "kanagawa-wave".into(),
            foreground: Color::rgb(0xDC, 0xD7, 0xBA),
            background: Color::rgb(0x1F, 0x1F, 0x28),
            ansi: [
                Color::rgb(0x16, 0x16, 0x1D),
                Color::rgb(0xC3, 0x40, 0x43),
                Color::rgb(0x76, 0x94, 0x6A),
                Color::rgb(0xC0, 0xA3, 0x6E),
                Color::rgb(0x7E, 0x9C, 0xD8),
                Color::rgb(0x95, 0x7F, 0xB8),
                Color::rgb(0x6A, 0x95, 0x89),
                Color::rgb(0xC8, 0xC0, 0x93),
                Color::rgb(0x72, 0x73, 0x69),
                Color::rgb(0xE8, 0x21, 0x24),
                Color::rgb(0x98, 0xBB, 0x6C),
                Color::rgb(0xE6, 0xC3, 0x84),
                Color::rgb(0x7F, 0xB4, 0xCA),
                Color::rgb(0x93, 0x8A, 0xA9),
                Color::rgb(0x7A, 0xA8, 0x9F),
                Color::rgb(0xDC, 0xD7, 0xBA),
            ],
        }
    }

    pub fn everforest_dark() -> Self {
        Self {
            name: "everforest-dark".into(),
            foreground: Color::rgb(0xD3, 0xC6, 0xAA),
            background: Color::rgb(0x27, 0x2E, 0x33),
            ansi: [
                Color::rgb(0x41, 0x4B, 0x50),
                Color::rgb(0xE6, 0x7E, 0x80),
                Color::rgb(0xA7, 0xC0, 0x80),
                Color::rgb(0xDB, 0xBC, 0x7F),
                Color::rgb(0x7F, 0xBB, 0xB3),
                Color::rgb(0xD6, 0x99, 0xB6),
                Color::rgb(0x83, 0xC0, 0x92),
                Color::rgb(0xD3, 0xC6, 0xAA),
                Color::rgb(0x9D, 0xA9, 0xA0),
                Color::rgb(0xE6, 0x7E, 0x80),
                Color::rgb(0xA7, 0xC0, 0x80),
                Color::rgb(0xDB, 0xBC, 0x7F),
                Color::rgb(0x7F, 0xBB, 0xB3),
                Color::rgb(0xD6, 0x99, 0xB6),
                Color::rgb(0x83, 0xC0, 0x92),
                Color::rgb(0xD3, 0xC6, 0xAA),
            ],
        }
    }

    pub fn everforest_light() -> Self {
        Self {
            name: "everforest-light".into(),
            foreground: Color::rgb(0x5C, 0x6A, 0x72),
            background: Color::rgb(0xEF, 0xEB, 0xD4),
            ansi: [
                Color::rgb(0x7A, 0x84, 0x78),
                Color::rgb(0xE6, 0x7E, 0x80),
                Color::rgb(0x9A, 0xB3, 0x73),
                Color::rgb(0xC1, 0xA2, 0x66),
                Color::rgb(0x7F, 0xBB, 0xB3),
                Color::rgb(0xD6, 0x99, 0xB6),
                Color::rgb(0x83, 0xC0, 0x92),
                Color::rgb(0xB2, 0xAF, 0x9F),
                Color::rgb(0xA6, 0xB0, 0xA0),
                Color::rgb(0xF8, 0x55, 0x52),
                Color::rgb(0x8D, 0xA1, 0x01),
                Color::rgb(0xDF, 0xA0, 0x00),
                Color::rgb(0x3A, 0x94, 0xC5),
                Color::rgb(0xDF, 0x69, 0xBA),
                Color::rgb(0x35, 0xA7, 0x7C),
                Color::rgb(0xFF, 0xFB, 0xEF),
            ],
        }
    }

    pub fn solarized_light() -> Self {
        Self {
            name: "solarized-light".into(),
            foreground: Color::rgb(0x65, 0x7B, 0x83),
            background: Color::rgb(0xFD, 0xF6, 0xE3),
            ansi: [
                Color::rgb(0x07, 0x36, 0x42),
                Color::rgb(0xDC, 0x32, 0x2F),
                Color::rgb(0x85, 0x99, 0x00),
                Color::rgb(0xB5, 0x89, 0x00),
                Color::rgb(0x26, 0x8B, 0xD2),
                Color::rgb(0xD3, 0x36, 0x82),
                Color::rgb(0x2A, 0xA1, 0x98),
                Color::rgb(0xBB, 0xB5, 0xA2),
                Color::rgb(0x00, 0x2B, 0x36),
                Color::rgb(0xCB, 0x4B, 0x16),
                Color::rgb(0x58, 0x6E, 0x75),
                Color::rgb(0x65, 0x7B, 0x83),
                Color::rgb(0x83, 0x94, 0x96),
                Color::rgb(0x6C, 0x71, 0xC4),
                Color::rgb(0x93, 0xA1, 0xA1),
                Color::rgb(0xFD, 0xF6, 0xE3),
            ],
        }
    }

    pub fn paper_light() -> Self {
        Self {
            name: "paper-light".into(),
            foreground: Color::rgb(0x14, 0x14, 0x13),
            background: Color::rgb(0xFA, 0xF9, 0xF5),
            ansi: [
                Color::rgb(0x3D, 0x3D, 0x3A),
                Color::rgb(0xE0, 0x5A, 0x52),
                Color::rgb(0x13, 0x93, 0x52),
                Color::rgb(0xA3, 0x72, 0x00),
                Color::rgb(0x35, 0x6A, 0xD1),
                Color::rgb(0x87, 0x41, 0xBB),
                Color::rgb(0x0E, 0x7E, 0x83),
                Color::rgb(0x59, 0x59, 0x53),
                Color::rgb(0x85, 0x85, 0x80),
                Color::rgb(0xE0, 0x5A, 0x52),
                Color::rgb(0x13, 0x93, 0x52),
                Color::rgb(0xA3, 0x72, 0x00),
                Color::rgb(0x35, 0x6A, 0xD1),
                Color::rgb(0x87, 0x41, 0xBB),
                Color::rgb(0x0E, 0x7E, 0x83),
                Color::rgb(0xFA, 0xF9, 0xF5),
            ],
        }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "flexoki-dark" | "flexoki" => Some(Self::flexoki_dark()),
            "flexoki-light" => Some(Self::flexoki_light()),
            "catppuccin-mocha" | "catppuccin" => Some(Self::catppuccin_mocha()),
            "tokyonight" | "tokyo-night" => Some(Self::tokyonight()),
            "dracula" => Some(Self::dracula()),
            "nord" => Some(Self::nord()),
            "rose-pine" | "rosepine" => Some(Self::rose_pine()),
            "gruvbox-dark" | "gruvbox" => Some(Self::gruvbox_dark()),
            "solarized-dark" | "solarized" => Some(Self::solarized_dark()),
            "solarized-light" => Some(Self::solarized_light()),
            "one-half-dark" | "onehalfdark" => Some(Self::one_half_dark()),
            "kanagawa-wave" | "kanagawa" => Some(Self::kanagawa_wave()),
            "everforest-dark" | "everforest" => Some(Self::everforest_dark()),
            "everforest-light" => Some(Self::everforest_light()),
            "paper-light" => Some(Self::paper_light()),
            _ => Self::load_user_themes()
                .into_iter()
                .find(|theme| theme.name == name.to_lowercase()),
        }
    }

    pub fn all_available() -> Vec<Self> {
        let mut themes: Vec<Self> = Self::available()
            .iter()
            .filter_map(|name| Self::by_name(name))
            .collect();
        for theme in Self::load_user_themes() {
            if !themes.iter().any(|existing| existing.name == theme.name) {
                themes.push(theme);
            }
        }
        themes
    }

    pub fn available() -> &'static [&'static str] {
        &[
            "flexoki-dark",
            "flexoki-light",
            "catppuccin-mocha",
            "tokyonight",
            "dracula",
            "nord",
            "rose-pine",
            "gruvbox-dark",
            "solarized-dark",
            "solarized-light",
            "one-half-dark",
            "kanagawa-wave",
            "everforest-dark",
            "everforest-light",
            "paper-light",
        ]
    }

    pub fn from_ghostty_format(name: &str, content: &str) -> Option<Self> {
        let mut background = None;
        let mut foreground = None;
        let mut ansi = [Color::rgb(0, 0, 0); 16];
        let mut palette_count = 0;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let mut parts = line.splitn(2, '=');
            let key = parts.next()?.trim();
            let value = parts.next()?.trim();

            match key {
                "background" => background = parse_hex_color(value),
                "foreground" => foreground = parse_hex_color(value),
                "palette" => {
                    let mut idx_color = value.splitn(2, '=');
                    if let (Some(idx_str), Some(hex)) = (idx_color.next(), idx_color.next())
                        && let Ok(idx) = idx_str.trim().parse::<usize>()
                        && idx < 16
                        && let Some(color) = parse_hex_color(hex.trim())
                    {
                        ansi[idx] = color;
                        palette_count += 1;
                    }
                }
                _ => {}
            }
        }

        if let (Some(foreground), Some(background)) = (foreground, background)
            && palette_count >= 8
        {
            Some(Self {
                name: name.to_string(),
                foreground,
                background,
                ansi,
            })
        } else {
            None
        }
    }

    pub fn load_user_themes() -> Vec<Self> {
        let dir = Self::user_themes_dir();

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        let mut themes = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("")
                .to_lowercase()
                .replace(' ', "-");
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Some(theme) = Self::from_ghostty_format(&name, &content)
            {
                themes.push(theme);
            }
        }

        themes
    }

    pub fn user_themes_dir() -> std::path::PathBuf {
        con_paths::user_themes_dir()
    }
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self::flexoki_dark()
    }
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_is_based_on_background_luminance_not_name() {
        let mut pale_theme = TerminalTheme::flexoki_dark();
        pale_theme.name = "definitely-dark".into();
        pale_theme.background = Color::rgb(0xFF, 0xF1, 0x9A);
        assert!(!pale_theme.is_dark());

        let mut tinted_theme = TerminalTheme::flexoki_light();
        tinted_theme.name = "definitely-light".into();
        tinted_theme.background = Color::rgb(0x16, 0x28, 0x36);
        assert!(tinted_theme.is_dark());
    }

    #[test]
    fn ghostty_format_round_trips_all_colors() {
        let theme = TerminalTheme::flexoki_light();
        let serialized = theme.to_ghostty_format();
        let parsed = TerminalTheme::from_ghostty_format("round-trip", &serialized).unwrap();

        assert_eq!(parsed.foreground, theme.foreground);
        assert_eq!(parsed.background, theme.background);
        assert_eq!(parsed.ansi, theme.ansi);
        assert_eq!(
            serialized
                .lines()
                .filter(|line| line.starts_with("palette"))
                .count(),
            16
        );
    }
}
