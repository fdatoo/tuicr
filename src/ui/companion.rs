mod markdown;
mod presentation;

pub(crate) use presentation::Presentation;

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::ui::styles;

use markdown::{readable_color, safe_display};
use presentation::render_transcript;

pub(super) fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(view) = &app.companion.view else {
        return;
    };
    if area.is_empty() {
        return;
    }
    let theme = &app.theme;
    let emphasis = styles::review_emphasis(theme);
    let history = view.history_visible;
    let title = if history {
        " Private review history ".into()
    } else if app.companion.pending_status().is_some() && area.height == 3 {
        " Private companion · Ctrl-x cancel ".into()
    } else if let Some(anchor) = view.anchor() {
        format!(" Private companion · {} ", safe_display(&anchor.label()))
    } else {
        " Private companion ".into()
    };
    let mut panel = Block::default().style(styles::panel_style(theme).patch(emphasis.muted));
    if area.height > 2 {
        panel = panel.title(title).borders(Borders::TOP);
    }
    let inner = panel.inner(area);
    frame.render_widget(panel, area);

    let bold = emphasis.primary.add_modifier(Modifier::BOLD);
    let stale = app.companion.view_is_stale();
    let composing = !history && view.anchor().is_some();
    let composer_height = if composing {
        (view.question.bytes().filter(|&b| b == b'\n').count() + 1)
            .min(3)
            .min(usize::from(inner.height.saturating_sub(2).max(1))) as u16
    } else {
        0
    };
    let mut body = inner;
    body.height = body.height.saturating_sub(composer_height);
    let busy = app.companion.busy_notice();
    let error = stale
        .then_some("This thread is about earlier code. Close and reopen the anchor.")
        .or(busy.as_deref())
        .or(view.error.as_deref())
        .or(app.companion.storage_warning.as_deref());
    if let Some(error) = error {
        let message =
            Paragraph::new(Line::styled(safe_display(error), bold)).wrap(Wrap { trim: true });
        let height = message
            .line_count(body.width)
            .min(usize::from(body.height.min(3))) as u16;
        frame.render_widget(message, take_top(&mut body, height));
    }
    if let Some(activity) = app.companion.pending_status()
        && !history
        && busy.is_none()
        && body.height > 0
    {
        frame.render_widget(
            Line::styled(
                format!("Ctrl-x cancel · {}", safe_display(activity)),
                emphasis.muted,
            ),
            take_top(&mut body, 1),
        );
    }
    if !history
        && body.height >= 6
        && view.guide_preview().is_none()
        && let Some(preview) = view.preview()
    {
        let row_limit = usize::from((body.height / 4).clamp(1, 3));
        let rows = app.companion.presentation.preview_rows(
            preview,
            theme,
            usize::from(body.width),
            row_limit,
        );
        let preview_area = take_top(&mut body, rows.len() as u16);
        let fg = readable_color(theme.fg_primary, theme.fg_primary, theme.bg_highlight);
        frame.render_widget(
            Block::default().style(styles::selected_style(theme).fg(fg)),
            preview_area,
        );
        for (offset, row) in rows.iter().enumerate() {
            frame.render_widget(
                row,
                Rect::new(
                    preview_area.x,
                    preview_area.y + offset as u16,
                    preview_area.width,
                    1,
                ),
            );
        }
    }

    let folded = !history
        && app
            .companion
            .history
            .iter()
            .rev()
            .find(|turn| view.matches_thread(&turn.anchor))
            .is_some_and(|turn| turn.short_preview().is_some() && !view.shows_full_answer(turn));
    let guided = !history && !stale && view.guide_preview().is_some();
    let hint = if history {
        "↑/↓ select · Enter jump · Tab thread · Esc close"
    } else if view.feedback.is_some() {
        "Ctrl-e edit draft · Esc close"
    } else if folded {
        "Ctrl-o show full answer · Esc close"
    } else if guided && view.followup_selected.is_some() && view.question.trim().is_empty() {
        "↑/↓ choose · Enter answer · Esc close"
    } else if !view.question.trim().is_empty() {
        "Enter send · Ctrl-d draft · Esc close"
    } else {
        "Ctrl-o more · Tab history · Esc close"
    };
    // Keep room for the two section labels, compact explanation and first
    // question before spending scarce rows on the input rule or key hints.
    let show_hint = body.height >= if guided { 6 } else { 2 };
    let show_rule = composing && body.height >= if guided { 7 } else { 4 };
    body.height -= u16::from(show_rule) + u16::from(show_hint);
    render_transcript(frame, body, &mut app.companion, theme, stale);
    let bottom = inner.bottom();
    if show_rule {
        let rule = Rect::new(inner.x, body.bottom(), inner.width, 1);
        frame.render_widget(
            Block::default()
                .borders(Borders::TOP)
                .border_style(emphasis.muted),
            rule,
        );
    }
    if composer_height > 0
        && let Some(view) = &app.companion.view
    {
        let y = bottom - composer_height - u16::from(show_hint);
        let composer = Rect::new(inner.x, y, inner.width, composer_height);
        let marker =
            styles::selection_marker_style(theme, Style::default().fg(theme.border_focused));
        render_question(
            frame,
            composer,
            &view.question,
            view.question_cursor,
            &emphasis,
            marker,
        );
    }
    if show_hint {
        let footer = Rect::new(inner.x, bottom - 1, inner.width, 1);
        frame.render_widget(Line::styled(hint, emphasis.muted), footer);
    }
}

fn take_top(area: &mut Rect, height: u16) -> Rect {
    let row = Rect::new(area.x, area.y, area.width, height);
    area.y += height;
    area.height -= height;
    row
}

fn render_question(
    frame: &mut Frame,
    area: Rect,
    question: &str,
    cursor: usize,
    emphasis: &styles::ReviewEmphasis,
    marker: Style,
) {
    let before_cursor = &question[..cursor];
    let cursor_row = before_cursor.bytes().filter(|&b| b == b'\n').count();
    let cursor_col = before_cursor.rsplit('\n').next().map_or(0, str::len);
    let first_row = (cursor_row + 1).saturating_sub(usize::from(area.height));
    let prefix = if area.width >= 16 {
        "Question: "
    } else if area.width >= 3 {
        "> "
    } else {
        ""
    };
    let width = usize::from(area.width).saturating_sub(prefix.len());
    for (row, text) in question
        .split('\n')
        .enumerate()
        .skip(first_row)
        .take(usize::from(area.height))
    {
        let y = area.y + (row - first_row) as u16;
        let is_active = row == cursor_row;
        let (start, caret) = if is_active {
            question_window(text, cursor_col, width)
        } else {
            (0, 0)
        };
        let label = if row == 0 || row == first_row {
            prefix
        } else {
            &"          "[..prefix.len()]
        };
        let label_tone = if question.is_empty() {
            emphasis.muted
        } else {
            emphasis.primary
        };
        let (label, punctuation) = label.split_at(label.len().saturating_sub(2));
        let line = Line::from(vec![
            Span::styled(label, label_tone),
            Span::styled(punctuation, marker),
            Span::styled(safe_display(&text[start..]), emphasis.primary),
        ]);
        frame.render_widget(line, Rect::new(area.x, y, area.width, 1));
        if is_active && width > 0 {
            frame.set_cursor_position(Position::new(area.x + (prefix.len() + caret) as u16, y));
        }
    }
}

fn question_window(text: &str, at: usize, width: usize) -> (usize, usize) {
    let mut start = at;
    let mut caret = 0;
    for (offset, grapheme) in text[..at].grapheme_indices(true).rev() {
        let next = caret + safe_display(grapheme).width();
        if next >= width {
            break;
        }
        start = offset;
        caret = next;
    }
    (start, caret)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ratatui::text::Line;

    use super::markdown;
    use super::presentation::preview_rows;
    use crate::app::companion::CompanionPreview;
    use crate::theme::Theme;
    use crate::ui::styles;

    fn plain_preview_rows(
        text: &str,
        truncated: bool,
        width: usize,
        row_limit: usize,
    ) -> Vec<Line<'static>> {
        preview_rows(
            CompanionPreview {
                text,
                truncated,
                source_path: None,
            },
            &Theme::dark(),
            width,
            row_limit,
        )
    }

    fn preview_text(rows: &[Line<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn companion_preview_identifies_unshown_selection_lines() {
        let rows = plain_preview_rows("first\nsecond\nthird\n", false, 35, 2);
        assert_eq!(preview_text(&rows), ["first", "second … (truncated)"]);
    }

    #[test]
    fn companion_preview_identifies_truncated_capture() {
        let rows = plain_preview_rows("first\n", true, 20, 3);
        assert_eq!(preview_text(&rows), ["first…"]);
    }

    #[test]
    fn companion_preview_clips_to_cells_without_splitting_wide_graphemes() {
        let rows = plain_preview_rows("a👩‍💻界", false, 4, 1);
        assert_eq!(preview_text(&rows), ["a👩‍💻…"]);
    }

    #[test]
    fn companion_preview_applies_grammar_only_to_source_paths() {
        let text = "let preview = 17;";
        let theme = Theme::dark();
        let source = preview_rows(
            CompanionPreview {
                text,
                truncated: false,
                source_path: Some(Path::new("source.rs")),
            },
            &theme,
            30,
            1,
        );
        let prose = plain_preview_rows(text, false, 30, 1);
        assert_eq!(preview_text(&source), [text]);
        assert_eq!(preview_text(&prose), [text]);
        assert_ne!(
            token_style(&source[0], "let").fg,
            token_style(&source[0], "17").fg
        );
        assert_eq!(prose[0].spans.len(), 1);
        assert!(prose[0].spans[0].style.fg.is_none());
    }

    #[test]
    fn companion_markdown_renders_structure_and_composes_inline_styles() {
        use ratatui::style::Modifier;

        let theme = Theme::dark();
        let rows = markdown::render(
            "# Findings\n\n**Strong and *emphasized*** [reference](https://example.invalid) `value` &amp; \\*literal\\*\n\
             continuation  \nnext line\n\n3. first\n4. second\n   - nested\n\n> quoted",
            &theme,
        );
        assert_eq!(
            preview_text(&rows),
            [
                "Findings",
                "",
                "Strong and emphasized reference value & *literal* continuation",
                "next line",
                "",
                "3. first",
                "4. second",
                "  • nested",
                "",
                "│ quoted",
            ],
        );
        assert!(
            token_style(&rows[0], "Findings")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            token_style(&rows[2], "emphasized")
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)
        );
        assert!(
            token_style(&rows[2], "reference")
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert_eq!(token_style(&rows[2], "value").fg, Some(theme.comment_note));
    }

    #[test]
    fn companion_markdown_highlights_fences_without_losing_code_or_literals() {
        let source = "```rust extra\nlet answer = 17;\n\n\tprintln!(\"界\");\n```\n\n\
             ```made-up\nlet plain = 3;\n```\n\n``` \nlet untagged = 4;\n```";
        let dark = markdown::render(source, &Theme::dark());
        let light = markdown::render(source, &Theme::light());
        assert_eq!(
            preview_text(&dark),
            [
                "  let answer = 17;",
                "  ",
                "      println!(\"界\");",
                "",
                "  let plain = 3;",
                "",
                "  let untagged = 4;",
            ],
        );
        assert_eq!(preview_text(&dark), preview_text(&light));
        let dark_keyword = token_style(&dark[0], "let").fg;
        assert!(dark_keyword.is_some());
        assert_ne!(dark_keyword, token_style(&dark[0], "17").fg);
        assert_ne!(dark_keyword, token_style(&light[0], "let").fg);
        assert!(token_style(&dark[4], "let plain = 3;").fg.is_none());
        assert!(token_style(&dark[6], "let untagged = 4;").fg.is_none());
    }

    #[test]
    fn companion_light_theme_code_is_readable_against_both_panel_surfaces() {
        use ratatui::style::Color;

        for theme in [Theme::light(), Theme::solarized_light()] {
            let source = "let status = 17; // delegated";
            let fenced = format!("`status`\n\n```rust\n{source}\n```");
            let rows = markdown::render(&fenced, &theme);
            assert_code_contrast(&rows, theme.panel_bg, theme.fg_primary);
            let preview = preview_rows(
                CompanionPreview {
                    text: source,
                    truncated: false,
                    source_path: Some(Path::new("source.rs")),
                },
                &theme,
                80,
                1,
            );
            assert_code_contrast(&preview, theme.bg_highlight, theme.fg_primary);
        }

        fn assert_code_contrast(rows: &[Line<'_>], background: Color, foreground: Color) {
            for span in rows.iter().flat_map(|line| &line.spans) {
                if span.content.trim().is_empty() {
                    continue;
                }
                let ink = span.style.fg.unwrap_or_else(|| {
                    markdown::readable_color(foreground, foreground, background)
                });
                let level = styles::contrast_ratio(
                    styles::relative_luminance(ink).unwrap(),
                    styles::relative_luminance(background).unwrap(),
                );
                assert!(
                    level >= 4.5,
                    "{:?}: {ink:?} on {background:?} = {level}",
                    span.content
                );
                assert!(
                    span.style.bg.is_none(),
                    "Code must use the dock's background"
                );
            }
        }
    }

    #[test]
    fn companion_markdown_emits_only_inert_display_text() {
        let raw =
            r#"[label](javascript:ignored) &amp; &#27;[31m <img src="https://example.invalid">"#;
        let rows = markdown::render(raw, &Theme::dark());
        let text = preview_text(&rows).join("\n");
        assert!(text.contains("label &"), "{text:?}");
        assert!(
            text.contains(r#"<img src="https://example.invalid">"#),
            "{text:?}"
        );
        assert!(!text.contains("javascript:ignored"), "{text:?}");
        assert!(!text.contains(char::is_control), "{text:?}");
        assert_eq!(markdown::safe_display("\t界👩‍💻\x1b"), "    界👩‍💻�");
    }

    fn token_style(row: &Line<'_>, token: &str) -> ratatui::style::Style {
        row.spans
            .iter()
            .find(|span| span.content.contains(token))
            .unwrap_or_else(|| panic!("{token:?} was not found in {row:?}"))
            .style
    }
}
