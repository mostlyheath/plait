//! Color themes. Plait draws from one `Palette` of named roles; two are tuned
//! by hand, and the rest are derived from Base16 schemes, a plain 16-color
//! format with hundreds of published schemes. A few ship with the app, and
//! scheme files placed in the themes folder are read too.
//!
//! Derived roles mix the scheme's background toward its text, so every theme
//! keeps the same hierarchy, and a contrast floor keeps text readable when a
//! scheme's own grays or accents are too faint against its background.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Every color role the desktop draws with. Opaque colors are `0xRRGGBB`;
/// translucent ones (`hover`, the fills, and the selections) are `0xRRGGBBAA`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub light: bool,
    pub bg: u32,
    /// The sidebar and other rails beside the main surface.
    pub rail: u32,
    /// Raised surfaces: the composer, cards, inline code.
    pub panel: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    /// Section labels, hints, and counts: quieter than `muted`, still readable.
    pub subtle: u32,
    pub mint: u32,
    pub blue: u32,
    pub violet: u32,
    pub red: u32,
    pub amber: u32,
    /// Selected rows. Neutral, so accents stay meaningful.
    pub selected: u32,
    /// Outline of raised controls such as the composer and approval card.
    pub edge: u32,
    /// Focus edge of a text input: quieter than the ring on small controls.
    pub focus_edge: u32,
    /// Translucent so one hover step reads the same on every surface.
    pub hover: u32,
    /// Selected conversation text.
    pub selection: u32,
    /// Selected text in an editable field.
    pub text_selection: u32,
    pub added_fill: u32,
    pub removed_fill: u32,
    pub failed_edge: u32,
    /// Hover on a filled control, and on the one primary control.
    pub raised_hover: u32,
    pub primary_hover: u32,
    /// Dims the window behind a modal.
    pub scrim: u32,
    /// Outline and fill of a caution card, such as a pending approval.
    pub warn_edge: u32,
    pub warn_fill: u32,
}

/// The default dark theme. Text is a soft off-white rather than near white,
/// which is easier on the eyes over long reading.
pub const PLAIT_DARK: Palette = Palette {
    light: false,
    bg: 0x1c1d20,
    rail: 0x202124,
    panel: 0x292b2f,
    border: 0x33353b,
    text: 0xd9dce3,
    muted: 0x9ba2af,
    subtle: 0x7d8492,
    mint: 0x8fd3bc,
    blue: 0x8fb2ec,
    violet: 0xb9a8e8,
    red: 0xe59ea5,
    amber: 0xdcbd88,
    selected: 0x2e3035,
    edge: 0x3d4048,
    focus_edge: 0x6b8bbf,
    hover: 0xffffff0b,
    selection: 0xffffff29,
    text_selection: 0x8fb2ec4d,
    added_fill: 0x8fd3bc1c,
    removed_fill: 0xe59ea51c,
    failed_edge: 0xe59ea566,
    raised_hover: 0x323439,
    primary_hover: 0xeceef2,
    scrim: 0x00000066,
    warn_edge: 0x5c4f38,
    warn_fill: 0x3b3224,
};

/// The default light theme: warm off-white, never pure white behind text.
pub const PLAIT_LIGHT: Palette = Palette {
    light: true,
    bg: 0xf7f7f5,
    rail: 0xefefec,
    panel: 0xffffff,
    border: 0xe3e3df,
    text: 0x2b2e33,
    muted: 0x5d636e,
    subtle: 0x737985,
    mint: 0x1d7a5a,
    blue: 0x2f62b8,
    violet: 0x6f4fc2,
    red: 0xb53a4c,
    amber: 0x8a5f0a,
    selected: 0xe7e7e3,
    edge: 0xd3d4cf,
    focus_edge: 0x6f93d6,
    hover: 0x0000000a,
    selection: 0x0000001f,
    text_selection: 0x2f62b833,
    added_fill: 0x1d7a5a1f,
    removed_fill: 0xb53a4c1a,
    failed_edge: 0xb53a4c66,
    raised_hover: 0xf2f2ef,
    primary_hover: 0x44484f,
    scrim: 0x00000040,
    warn_edge: 0xd9c39a,
    warn_fill: 0xf6ecd8,
};

/// Stillwater (github.com/heath0xFF/stillwater, MIT), a quiet indigo-slate dark
/// theme, mapped by hand from its palette: frost cyan for types, links, and
/// focus; orchid for keywords; its comment gray for hints.
pub const STILLWATER: Palette = Palette {
    light: false,
    bg: 0x1f222e,
    rail: 0x242835,
    panel: 0x2b303f,
    border: 0x313547,
    text: 0xdfe3ed,
    muted: 0xb0b7cb,
    subtle: 0x9da5bf,
    mint: 0x70d689,
    blue: 0x74cde2,
    violet: 0xec8ac7,
    red: 0xec7368,
    amber: 0xeec96e,
    selected: 0x2e3243,
    edge: 0x464d66,
    focus_edge: 0x5a9aac,
    hover: 0xffffff0b,
    selection: 0x464d66c0,
    text_selection: 0x74cde24d,
    added_fill: 0x70d6891c,
    removed_fill: 0xec73681c,
    failed_edge: 0xec736866,
    raised_hover: 0x32384a,
    primary_hover: 0xeef0f6,
    scrim: 0x00000066,
    warn_edge: 0x675c44,
    warn_fill: 0x3c3937,
};

pub const PLAIT_DARK_NAME: &str = "Plait Dark";
pub const STILLWATER_NAME: &str = "Stillwater";
/// What unset preferences draw with: Stillwater in the dark, Plait Light by day.
pub const DEFAULT_DARK: Palette = STILLWATER;
pub const DEFAULT_DARK_NAME: &str = STILLWATER_NAME;
pub const DEFAULT_LIGHT: Palette = PLAIT_LIGHT;
pub const DEFAULT_LIGHT_NAME: &str = PLAIT_LIGHT_NAME;
pub const PLAIT_LIGHT_NAME: &str = "Plait Light";

/// Base16 schemes shipped with the app, from tinted-theming/schemes (MIT);
/// see `assets/themes/base16/README.md` for the revision.
const BUNDLED: [&str; 15] = [
    include_str!("../../../assets/themes/base16/catppuccin-latte.yaml"),
    include_str!("../../../assets/themes/base16/catppuccin-mocha.yaml"),
    include_str!("../../../assets/themes/base16/everforest-dark-medium.yaml"),
    include_str!("../../../assets/themes/base16/everforest-light-medium.yaml"),
    include_str!("../../../assets/themes/base16/gruvbox-dark-medium.yaml"),
    include_str!("../../../assets/themes/base16/gruvbox-light-medium.yaml"),
    include_str!("../../../assets/themes/base16/nord.yaml"),
    include_str!("../../../assets/themes/base16/one-light.yaml"),
    include_str!("../../../assets/themes/base16/onedark.yaml"),
    include_str!("../../../assets/themes/base16/rose-pine-dawn.yaml"),
    include_str!("../../../assets/themes/base16/rose-pine.yaml"),
    include_str!("../../../assets/themes/base16/solarized-dark.yaml"),
    include_str!("../../../assets/themes/base16/solarized-light.yaml"),
    include_str!("../../../assets/themes/base16/tokyo-night-dark.yaml"),
    include_str!("../../../assets/themes/base16/tokyo-night-light.yaml"),
];

/// Scheme files larger than this are not themes.
const FILE_LIMIT: u64 = 64 * 1024;
/// Files read from the themes folder at most.
const FOLDER_LIMIT: usize = 200;

/// Whether the window follows macOS or keeps one appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    BuiltIn,
    /// A theme made elsewhere and mapped by hand, with its credit.
    Included {
        credit: &'static str,
    },
    /// Shipped Base16 scheme, with its credited author.
    Bundled {
        author: String,
    },
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub palette: Palette,
    pub source: Source,
}

/// A parsed Base16 scheme: its name, whether it is light, and base00–base0F.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scheme {
    pub name: String,
    pub author: String,
    pub light: bool,
    pub base: [u32; 16],
}

/// The two hand-tuned themes, then the bundled schemes by name.
pub fn built_in() -> Vec<Theme> {
    // Each list starts with its default.
    let mut themes = vec![
        Theme {
            name: STILLWATER_NAME.into(),
            palette: STILLWATER,
            source: Source::Included {
                credit: "Stillwater by heath0xFF (MIT)",
            },
        },
        Theme {
            name: PLAIT_DARK_NAME.into(),
            palette: PLAIT_DARK,
            source: Source::BuiltIn,
        },
        Theme {
            name: PLAIT_LIGHT_NAME.into(),
            palette: PLAIT_LIGHT,
            source: Source::BuiltIn,
        },
    ];
    let mut bundled: Vec<Theme> = BUNDLED
        .iter()
        .filter_map(|text| parse_base16(text).ok())
        .map(|scheme| Theme {
            name: scheme.name.clone(),
            palette: derive(&scheme),
            source: Source::Bundled {
                author: scheme.author.clone(),
            },
        })
        .collect();
    bundled.sort_by_key(|theme| theme.name.to_lowercase());
    themes.extend(bundled);
    themes
}

/// The folder read for scheme files, beside the desktop preferences.
pub fn folder() -> Option<PathBuf> {
    Some(crate::preferences::default_path()?.parent()?.join("themes"))
}

/// Scheme files in `dir`, sorted by name, and a message for each file that
/// could not be read. A missing folder is empty, not an error. Blocking.
pub fn load_folder(dir: &Path) -> (Vec<Theme>, Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Default::default(),
        Err(error) => {
            return (
                Vec::new(),
                vec![format!("Cannot read the themes folder: {error}")],
            );
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml")
            })
        })
        .collect();
    paths.sort();
    let mut themes = Vec::new();
    let mut errors = Vec::new();
    if paths.len() > FOLDER_LIMIT {
        errors.push(format!(
            "Only the first {FOLDER_LIMIT} theme files were read."
        ));
        paths.truncate(FOLDER_LIMIT);
    }
    for path in paths {
        let label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match read_scheme(&path) {
            Ok(scheme) => themes.push(Theme {
                name: scheme.name.clone(),
                palette: derive(&scheme),
                source: Source::File(path),
            }),
            Err(error) => errors.push(format!("{label}: {error}")),
        }
    }
    themes.sort_by_key(|theme| theme.name.to_lowercase());
    (themes, errors)
}

fn read_scheme(path: &Path) -> Result<Scheme, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > FILE_LIMIT {
        return Err("larger than a scheme file can be".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_base16(&text)
}

/// Read a Base16 (or Base24) scheme in either published layout: the current
/// one, with colors under `palette:`, or the older flat one with `scheme:`.
/// Only the keys Plait needs are read; anything else is ignored.
pub fn parse_base16(text: &str) -> Result<Scheme, String> {
    let mut name = None;
    let mut author = String::new();
    let mut variant = None;
    let mut base = [None; 16];
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = unquote(value);
        match key {
            "name" | "scheme" => name = Some(value.to_owned()),
            "author" => author = value.to_owned(),
            "variant" => variant = Some(value.eq_ignore_ascii_case("light")),
            _ => {
                let Some(digit) = key.strip_prefix("base0") else {
                    continue;
                };
                let Ok(index) = usize::from_str_radix(digit, 16) else {
                    continue;
                };
                if digit.len() == 1 {
                    base[index] = Some(hex(value).ok_or_else(|| format!("{key} is not a color"))?);
                }
            }
        }
    }
    let name = name
        .filter(|name| !name.is_empty())
        .ok_or("no scheme name")?;
    let mut colors = [0; 16];
    for (index, color) in base.iter().enumerate() {
        colors[index] = color.ok_or_else(|| format!("missing base0{index:X}"))?;
    }
    // Older files have no variant; a light background makes a light theme.
    let light = variant.unwrap_or_else(|| luminance(colors[0]) > 0.4);
    Ok(Scheme {
        name,
        author,
        light,
        base: colors,
    })
}

/// The value after `key:`, without quotes or a trailing comment.
fn unquote(value: &str) -> &str {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('"') {
        return rest.split('"').next().unwrap_or_default();
    }
    if let Some(rest) = value.strip_prefix('\'') {
        return rest.split('\'').next().unwrap_or_default();
    }
    // An unquoted comment starts at " #"; a color's own "#" comes first.
    value.split(" #").next().unwrap_or_default().trim()
}

fn hex(value: &str) -> Option<u32> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    (digits.len() == 6)
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

/// Plait's roles from a scheme: base00 behind base05 text, with base08–base0E
/// as accents. Grays come from mixing background toward text, not from the
/// scheme's own grays, whose roles vary from scheme to scheme.
pub fn derive(scheme: &Scheme) -> Palette {
    let [bg, rail, ..] = scheme.base;
    // Body text at least 7:1 (WCAG AAA) on both the main surface and request
    // panels, moving toward the scheme's strongest foreground.
    let mut text = floor(scheme.base[5], &[bg], scheme.base[7], 7.0);
    let mut panel = mix(bg, text, 0.06);
    if contrast(text, panel) < 7.0 {
        text = floor(text, &[bg, panel], scheme.base[7], 7.0);
        panel = mix(bg, text, 0.06);
        text = floor(text, &[bg, panel], scheme.base[7], 7.0);
    }
    let shade = |amount| mix(bg, text, amount);
    let accent = |index: usize| floor(scheme.base[index], &[bg, panel], text, 3.0);
    let (mint, blue, violet, red, amber) = (
        accent(0xB),
        accent(0xD),
        accent(0xE),
        accent(0x8),
        accent(0xA),
    );
    let light = scheme.light;
    Palette {
        light,
        bg,
        rail,
        panel,
        border: shade(0.12),
        text,
        muted: floor(shade(0.68), &[bg, panel], text, 4.5),
        subtle: floor(shade(0.52), &[bg, panel], text, 3.2),
        mint,
        blue,
        violet,
        red,
        amber,
        selected: shade(0.10),
        edge: shade(0.18),
        focus_edge: mix(bg, blue, 0.75),
        hover: alpha(text, if light { 0x0a } else { 0x0b }),
        selection: alpha(text, if light { 0x1f } else { 0x29 }),
        text_selection: alpha(blue, if light { 0x33 } else { 0x4d }),
        added_fill: alpha(mint, 0x1f),
        removed_fill: alpha(red, 0x1c),
        failed_edge: alpha(red, 0x66),
        raised_hover: shade(0.10),
        primary_hover: mix(text, bg, 0.12),
        scrim: if light { 0x00000040 } else { 0x00000066 },
        warn_edge: mix(bg, amber, 0.35),
        warn_fill: mix(bg, amber, 0.14),
    }
}

/// `color` moved toward `toward` until it has `ratio` contrast with every
/// surface in `surfaces`; toward black or white when `toward` itself falls short.
fn floor(color: u32, surfaces: &[u32], toward: u32, ratio: f64) -> u32 {
    let extreme = if luminance(surfaces[0]) > 0.18 {
        0x000000
    } else {
        0xffffff
    };
    let toward = if surfaces.iter().all(|&bg| contrast(toward, bg) >= ratio) {
        toward
    } else {
        extreme
    };
    let mut amount = 0.;
    let mut result = color;
    while surfaces.iter().any(|&bg| contrast(result, bg) < ratio) && amount < 1. {
        amount += 0.05;
        result = mix(color, toward, amount);
    }
    result
}

/// `a` mixed with `b` by `amount` (0 is `a`, 1 is `b`), per channel.
pub fn mix(a: u32, b: u32, amount: f64) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f64, ((b >> shift) & 0xff) as f64);
        ((x + (y - x) * amount).round() as u32).min(0xff) << shift
    };
    channel(16) | channel(8) | channel(0)
}

fn alpha(color: u32, alpha: u32) -> u32 {
    (color << 8) | alpha
}

/// Relative luminance, as WCAG defines it.
pub fn luminance(color: u32) -> f64 {
    let linear = |shift: u32| {
        let c = ((color >> shift) & 0xff) as f64 / 255.;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0)
}

/// WCAG contrast ratio between two opaque colors, from 1 to 21.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_published_layouts_and_ignores_comments() {
        let current =
            "system: \"base16\"\nname: \"Test\"\nauthor: \"someone\"\nvariant: \"dark\"\npalette:\n"
                .to_owned()
                + &(0..16)
                    .map(|i| format!("  base0{i:X}: \"#{:02x}{:02x}{:02x}\" # note\n", i, i, i))
                    .collect::<String>();
        let scheme = parse_base16(&current).unwrap();
        assert_eq!(scheme.name, "Test");
        assert!(!scheme.light);
        assert_eq!(scheme.base[0xA], 0x0a0a0a);
        let older = "scheme: 'Old'\nauthor: x\n".to_owned()
            + &(0..16)
                .map(|i| format!("base0{i:x}: f0f0f{i:x}\n"))
                .collect::<String>();
        let scheme = parse_base16(&older).unwrap();
        assert_eq!(scheme.name, "Old");
        // No variant, light background: a light theme.
        assert!(scheme.light);
        assert_eq!(scheme.base[0xf], 0xf0f0ff);
    }

    #[test]
    fn a_broken_scheme_names_what_is_wrong() {
        assert_eq!(
            parse_base16("base00: \"#000000\"").unwrap_err(),
            "no scheme name"
        );
        assert_eq!(
            parse_base16("name: x\nbase00: \"#00000\"").unwrap_err(),
            "base00 is not a color"
        );
        assert_eq!(
            parse_base16("name: x\nbase00: 000000").unwrap_err(),
            "missing base01"
        );
    }

    #[test]
    fn every_shipped_theme_keeps_text_readable() {
        let themes = built_in();
        assert_eq!(
            themes.len(),
            3 + BUNDLED.len(),
            "every bundled scheme parses"
        );
        for theme in themes {
            let p = theme.palette;
            let name = &theme.name;
            for (surface_name, surface) in [("bg", p.bg), ("panel", p.panel)] {
                assert!(
                    contrast(p.text, surface) >= 7.,
                    "{name} text on {surface_name}"
                );
                assert!(
                    contrast(p.muted, surface) >= 4.5,
                    "{name} muted on {surface_name}"
                );
                assert!(
                    contrast(p.subtle, surface) >= 3.,
                    "{name} subtle on {surface_name}"
                );
                for accent in [p.mint, p.blue, p.violet, p.red, p.amber] {
                    assert!(
                        contrast(accent, surface) >= 3.,
                        "{name} accent {accent:06x} on {surface_name}"
                    );
                }
            }
            assert_eq!(
                p.light,
                luminance(p.bg) > luminance(p.text),
                "{name} variant"
            );
        }
    }

    #[test]
    fn the_dark_text_is_softer_than_near_white() {
        for palette in [DEFAULT_DARK, PLAIT_DARK] {
            let ratio = contrast(palette.text, palette.bg);
            assert!((10.0..13.5).contains(&ratio), "{ratio}");
        }
        assert_eq!(built_in()[0].name, DEFAULT_DARK_NAME);
    }

    #[test]
    fn the_folder_reads_schemes_and_reports_bad_files() {
        let dir = std::env::temp_dir().join(format!("plait-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("good.yaml"), BUNDLED[6]).unwrap();
        std::fs::write(dir.join("bad.yml"), "name: Broken").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let (themes, errors) = load_folder(&dir);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Nord");
        assert_eq!(errors, vec!["bad.yml: missing base00".to_owned()]);
        assert!(load_folder(&dir.join("missing")).0.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
