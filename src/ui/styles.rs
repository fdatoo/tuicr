use ratatui::style::{Color, Modifier, Style};

use crate::model::LineOrigin;
use crate::theme::Theme;

pub fn selected_style(theme: &Theme) -> Style {
    Style::default().bg(theme.bg_highlight).fg(theme.fg_primary)
}

pub fn dim_style(theme: &Theme) -> Style {
    Style::default().fg(theme.fg_dim)
}

pub(crate) struct ReviewEmphasis {
    pub primary: Style,
    pub secondary: Style,
    pub muted: Style,
}

pub(crate) fn review_emphasis(theme: &Theme) -> ReviewEmphasis {
    ReviewEmphasis {
        primary: Style::default().fg(theme.fg_primary),
        secondary: Style::default().fg(theme.fg_secondary),
        muted: dim_style(theme),
    }
}

pub(crate) fn review_warning(theme: &Theme) -> Style {
    // The warning badge's foreground is often black; `pending` is the
    // theme's warning ink designed for use directly on the panel.
    Style::default().fg(theme.pending)
}

pub fn diff_add_style(theme: &Theme) -> Style {
    Style::default().fg(theme.diff_add).bg(theme.diff_add_bg)
}

pub fn diff_del_style(theme: &Theme) -> Style {
    Style::default().fg(theme.diff_del).bg(theme.diff_del_bg)
}

pub fn diff_context_style(theme: &Theme) -> Style {
    Style::default().fg(theme.diff_context)
}

/// Live background a syntax-highlighted diff span should show for a line of
/// the given origin. `DiffLine.highlighted_spans` bakes in whatever
/// `syntax_add_bg`/`syntax_del_bg` was active when the diff was loaded (or
/// last rehighlighted); this lets renderers override that per-frame from the
/// currently active theme instead of trusting the cached span's own
/// background, so add/del backgrounds never lag behind the active theme even
/// if a rehighlight was skipped or missed. `None` for context lines, which
/// never carry a baked-in background (`syntax::apply_diff_background`
/// intentionally excludes them).
pub fn diff_syntax_bg(theme: &Theme, origin: LineOrigin) -> Option<Color> {
    match origin {
        LineOrigin::Addition => Some(theme.syntax_add_bg),
        LineOrigin::Deletion => Some(theme.syntax_del_bg),
        LineOrigin::Context => None,
    }
}

/// Style of the change bar (`▌`) and the gap after it. A syntax-colored line
/// sits on `syntax_add_bg`/`syntax_del_bg`, so its bar does too and the row
/// keeps one background; an uncolored line keeps `diff_add_bg`/`diff_del_bg`.
pub fn diff_marker_style(theme: &Theme, origin: LineOrigin, highlighted: bool) -> Style {
    let style = match origin {
        LineOrigin::Addition => diff_add_style(theme),
        LineOrigin::Deletion => diff_del_style(theme),
        LineOrigin::Context => return diff_context_style(theme),
    };
    if highlighted {
        patch_highlighted_span_bg(style, theme, origin)
    } else {
        style
    }
}

/// Apply `diff_syntax_bg` to `style`, leaving it untouched when there is no
/// live override for `origin` (context lines).
pub fn patch_highlighted_span_bg(style: Style, theme: &Theme, origin: LineOrigin) -> Style {
    match diff_syntax_bg(theme, origin) {
        Some(bg) => style.bg(bg),
        None => style,
    }
}

pub fn expanded_context_style(theme: &Theme) -> Style {
    Style::default().fg(theme.expanded_context_fg)
}

pub fn diff_hunk_header_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.fg_dim)
        .bg(theme.section_highlight_bg())
}

pub fn file_header_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.fg_primary)
        .add_modifier(Modifier::BOLD)
}

pub fn reviewed_style(theme: &Theme) -> Style {
    Style::default().fg(theme.reviewed)
}

pub fn pending_style(theme: &Theme) -> Style {
    Style::default().fg(theme.pending)
}

pub fn border_style(theme: &Theme, focused: bool) -> Style {
    if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border_unfocused)
    }
}

pub fn panel_style(theme: &Theme) -> Style {
    Style::default().bg(theme.panel_bg).fg(theme.fg_primary)
}

pub(super) fn contrast_ratio(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub(super) fn relative_luminance(color: Color) -> Option<f64> {
    linear_rgb(color).map(luminance)
}

fn srgb(color: Color) -> Option<[u8; 3]> {
    Some(match color {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Black => [0, 0, 0],
        Color::White => [255, 255, 255],
        Color::Indexed(index @ 16..=231) => {
            let n = index - 16;
            let component = |n: u8| if n == 0 { 0 } else { 55 + 40 * n };
            [component(n / 36), component(n / 6 % 6), component(n % 6)]
        }
        Color::Indexed(index @ 232..=255) => [8 + 10 * (index - 232); 3],
        // The first 16 ANSI colors and the default foreground/background can
        // be remapped by the terminal. Do not invent RGB values for them.
        _ => return None,
    })
}

pub(super) fn linear_rgb(color: Color) -> Option<[f64; 3]> {
    Some(srgb(color)?.map(|n| {
        let n = f64::from(n) / 255.0;
        if n <= 0.04045 {
            n / 12.92
        } else {
            ((n + 0.055) / 1.055).powf(2.4)
        }
    }))
}

pub(super) fn luminance([r, g, b]: [f64; 3]) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn mix_toward(source: [u8; 3], panel: [u8; 3], percent: u8) -> Color {
    let mix = |a, b| {
        let percent = u16::from(percent);
        ((u16::from(a) * (100 - percent) + u16::from(b) * percent + 50) / 100) as u8
    };
    Color::Rgb(
        mix(source[0], panel[0]),
        mix(source[1], panel[1]),
        mix(source[2], panel[2]),
    )
}

/// Dim against the panel, but don't push already-muted ink below its original
/// contrast or otherwise push readable ink below 4.5:1.
pub(super) fn guide_surround_colors(fg: Color, bg: Color, panel: Color) -> (Color, Color) {
    let Some(panel_rgb) = srgb(panel) else {
        return (fg, bg);
    };
    let muted_bg = srgb(bg).map_or(bg, |rgb| mix_toward(rgb, panel_rgb, 50));
    let rendered_luma =
        |color| relative_luminance(if color == Color::Reset { panel } else { color });
    let (Some(ink), Some(ink_luma), Some(old_luma), Some(new_luma)) = (
        srgb(fg),
        relative_luminance(fg),
        rendered_luma(bg),
        rendered_luma(muted_bg),
    ) else {
        return (fg, muted_bg);
    };
    let target = 4.5_f64.min(contrast_ratio(ink_luma, old_luma));
    if contrast_ratio(ink_luma, new_luma) < target {
        return (fg, bg);
    }
    // Test real terminal RGB colors after rounding, and leave very quiet
    // text alone if it is already at the contrast floor.
    let mut lo = 0;
    let mut hi = 51;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        let mixed = mix_toward(ink, panel_rgb, mid);
        let luma = relative_luminance(mixed).expect("RGB blend has a luminance");
        if contrast_ratio(luma, new_luma) >= target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let muted_fg = if lo == 0 {
        fg
    } else {
        mix_toward(ink, panel_rgb, lo)
    };
    (muted_fg, muted_bg)
}

pub fn popup_style(theme: &Theme) -> Style {
    panel_style(theme)
}

pub fn status_bar_style(theme: &Theme) -> Style {
    Style::default()
        .bg(theme.status_bar_bg)
        .fg(theme.fg_primary)
}

pub fn mode_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.mode_fg)
        .bg(theme.mode_bg)
        .add_modifier(Modifier::BOLD)
}

pub fn file_status_style(theme: &Theme, status: char) -> Style {
    let color = match status {
        'A' => theme.file_added,
        'M' => theme.file_modified,
        'D' => theme.file_deleted,
        'R' => theme.file_renamed,
        _ => theme.fg_secondary,
    };
    Style::default().fg(color)
}

/// Override only the marker foreground, preserving the caller's legacy style.
pub fn selection_marker_style(theme: &Theme, fallback: Style) -> Style {
    match theme.selection_marker {
        Some(color) => fallback.fg(color),
        None => fallback,
    }
}

pub fn current_line_indicator_style(theme: &Theme) -> Style {
    selection_marker_style(theme, Style::default().fg(theme.border_focused))
}

pub fn hash_style(theme: &Theme) -> Style {
    Style::default().fg(theme.cursor_color)
}

pub fn branch_style(theme: &Theme) -> Style {
    Style::default().fg(theme.branch_name)
}

pub fn dir_icon_style(theme: &Theme) -> Style {
    Style::default().fg(theme.diff_hunk_header)
}

pub fn comment_type_style(_theme: &Theme, color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

pub fn comment_border_style(theme: &Theme, _color: Color) -> Style {
    // Match the file-header separator look so the comment box reads as a
    // structural divider rather than as colour-coded chrome. The comment-
    // type colour still lives on the [NOTE]/[ISSUE]/... label inside.
    file_header_style(theme)
}

/// Fixed palette used to tint comment chrome by author. Excludes red/green so
/// the colour never collides with diff add/del semantics. Cyan/yellow/magenta/
/// blue/light-magenta/light-cyan give us six visually distinct slots — enough
/// for a handful of agents alongside the human reviewer.
const AUTHOR_PALETTE: &[Color] = &[
    Color::Cyan,
    Color::Yellow,
    Color::Magenta,
    Color::Blue,
    Color::LightMagenta,
    Color::LightCyan,
];

/// Deterministic palette colour for a given author name. Always returns
/// a colour; callers gate visibility separately via [`author_accent`].
/// Hashes via FNV-1a so the mapping is platform-independent and survives
/// across runs.
pub fn author_color_for(author: &str) -> Color {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in author.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    AUTHOR_PALETTE[(hash as usize) % AUTHOR_PALETTE.len()]
}

/// `Some(colour)` when `author` differs from the viewer (so the comment
/// chrome should advertise authorship), `None` when the comment is the
/// viewer's own.
pub fn author_accent(viewer: &str, author: &str) -> Option<Color> {
    if author == viewer {
        None
    } else {
        Some(author_color_for(author))
    }
}

pub fn visual_selection_style(theme: &Theme) -> Style {
    Style::default().bg(theme.bg_highlight)
}

pub fn search_match_style(theme: &Theme) -> Style {
    Style::default().bg(theme.search_match_bg)
}

pub fn help_indicator_style(theme: &Theme) -> Style {
    Style::default().fg(theme.help_indicator).bg(theme.panel_bg)
}

pub fn range_bar_style(theme: &Theme) -> Style {
    current_line_indicator_style(theme)
}

pub fn error_inline_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.message_error_fg)
        .add_modifier(Modifier::BOLD)
}

pub fn pseudo_commit_tag_style(theme: &Theme) -> Style {
    Style::default().fg(theme.file_modified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_emphasis_orders_contrast_and_keeps_inline_warnings_legible() {
        for theme in [Theme::dark(), Theme::light()] {
            let tone = review_emphasis(&theme);
            let panel = relative_luminance(theme.palette_panel_bg()).unwrap();
            let contrast = |style: Style| {
                contrast_ratio(panel, relative_luminance(style.fg.unwrap()).unwrap())
            };
            assert!(contrast(tone.primary) > contrast(tone.secondary));
            assert!(contrast(tone.secondary) > contrast(tone.muted));
            assert!(contrast(tone.muted) >= 4.5);
            assert!(contrast(review_warning(&theme)) >= 4.5);
        }
    }

    /// A syntax-colored line's change bar takes the syntax background; an
    /// uncolored line's keeps the plain diff background.
    #[test]
    fn diff_marker_style_follows_the_line_background() {
        let theme = Theme::catppuccin_macchiato();

        let colored = diff_marker_style(&theme, LineOrigin::Addition, true);
        assert_eq!(colored.bg, Some(theme.syntax_add_bg));
        assert_eq!(colored.fg, Some(theme.diff_add));
        assert_eq!(
            diff_marker_style(&theme, LineOrigin::Deletion, true).bg,
            Some(theme.syntax_del_bg)
        );
        assert_eq!(
            diff_marker_style(&theme, LineOrigin::Addition, false),
            diff_add_style(&theme)
        );
        assert_eq!(
            diff_marker_style(&theme, LineOrigin::Context, true),
            diff_context_style(&theme)
        );
    }

    #[test]
    fn selection_marker_changes_only_marker_foregrounds() {
        let mut theme = Theme::github_dark();
        let legacy_pointer = selected_style(&theme);
        assert_eq!(
            selection_marker_style(&theme, legacy_pointer),
            legacy_pointer
        );
        assert_eq!(
            current_line_indicator_style(&theme).fg,
            Some(theme.border_focused)
        );
        assert_eq!(range_bar_style(&theme).fg, Some(theme.border_focused));
        let unchanged = (
            border_style(&theme, true),
            hash_style(&theme),
            selected_style(&theme),
            visual_selection_style(&theme),
        );

        theme.selection_marker = Some(Color::Rgb(36, 45, 56));
        assert_eq!(
            current_line_indicator_style(&theme).fg,
            theme.selection_marker
        );
        assert_eq!(range_bar_style(&theme).fg, theme.selection_marker);
        assert_eq!(
            selection_marker_style(&theme, legacy_pointer).bg,
            legacy_pointer.bg
        );
        assert_eq!(
            unchanged,
            (
                border_style(&theme, true),
                hash_style(&theme),
                selected_style(&theme),
                visual_selection_style(&theme)
            )
        );
    }
}
