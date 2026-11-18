use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use unicode_width::UnicodeWidthStr;

use crate::app::companion::{CompanionPreview, CompanionState, GUIDE_PROVIDER};
use crate::theme::Theme;
use crate::ui::styles;
use crate::ui::text_utils::{prefix_within_width, wrap_spans};

use super::markdown::{self, safe_display, syntax_spans};

/// Display-only data. Answers are immutable and append-only within a review;
/// the App drops these rows on review/theme changes, never in the composer.
#[derive(Default)]
pub(crate) struct Presentation {
    answers: Vec<Option<Vec<Line<'static>>>>,
    short_answers: Vec<Option<Vec<Line<'static>>>>,
    transcript: Option<Transcript>,
    preview: Option<Preview>,
}

impl Presentation {
    pub(crate) fn reset_transcript(&mut self) {
        self.transcript = None;
    }

    pub(crate) fn reset_view(&mut self) {
        self.reset_transcript();
        self.preview = None;
    }

    pub(super) fn preview_rows(
        &mut self,
        preview: CompanionPreview<'_>,
        theme: &Theme,
        width: usize,
        row_limit: usize,
    ) -> &[Line<'static>] {
        if self
            .preview
            .as_ref()
            .map(|cached| (cached.width, cached.row_limit))
            != Some((width, row_limit))
        {
            self.preview = Some(Preview {
                width,
                row_limit,
                rows: preview_rows(preview, theme, width, row_limit),
            });
        }
        &self.preview.as_ref().expect("preview was populated").rows
    }
}

struct Preview {
    width: usize,
    row_limit: usize,
    rows: Vec<Line<'static>>,
}

struct Transcript {
    width: usize,
    density: TranscriptDensity,
    latest: Option<usize>,
    selected_detail: Option<usize>,
    rows: Vec<TranscriptRow>,
    selected: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TranscriptDensity {
    Bare,
    Tight,
    Compact,
    Full,
}

impl TranscriptDensity {
    fn for_height(height: u16) -> Self {
        match height {
            0..=1 => Self::Bare,
            2..=3 => Self::Tight,
            4..=7 => Self::Compact,
            _ => Self::Full,
        }
    }
}

// Most answer rows fit without wrapping. Refer to those rows instead of
// keeping a second copy of the rendered answers in the active transcript.
enum TranscriptRow {
    Answer {
        exchange: usize,
        row: usize,
        folded: bool,
    },
    Display(Line<'static>),
}

pub(super) fn render_transcript(
    frame: &mut Frame,
    area: Rect,
    state: &mut CompanionState,
    theme: &Theme,
    stale: bool,
) {
    let gutter = u16::from(area.width >= 2);
    let width = usize::from(area.width - gutter);
    let density = TranscriptDensity::for_height(area.height);
    if area.is_empty() {
        return;
    }
    if state
        .presentation
        .transcript
        .as_ref()
        .map(|layout| (layout.width, layout.density))
        != Some((width, density))
    {
        state.presentation.transcript = Some(build_transcript(state, theme, width, stale, density));
    }
    let Some(view) = &mut state.view else {
        return;
    };
    let layout = state
        .presentation
        .transcript
        .as_ref()
        .expect("transcript was populated");
    let emphasis = styles::review_emphasis(theme);
    view.page_height = usize::from(area.height);
    view.max_scroll = layout.rows.len().saturating_sub(view.page_height);
    let selected = if view.history_visible {
        Some(view.history_selected)
    } else {
        layout.selected
    };
    if view.follow_tail {
        view.scroll = view.max_scroll;
    } else if let Some(selected) = selected {
        let last = layout.selected_detail.unwrap_or(selected);
        let minimum = (last + 1).saturating_sub(view.page_height);
        view.scroll = view.scroll.clamp(minimum.min(selected), selected);
    }
    view.scroll = view.scroll.min(view.max_scroll);
    for (offset, row) in layout
        .rows
        .iter()
        .skip(view.scroll)
        .take(view.page_height)
        .enumerate()
    {
        let row_area = Rect::new(area.x, area.y + offset as u16, width as u16, 1);
        let mut older_answer = false;
        let row = match row {
            TranscriptRow::Answer {
                exchange,
                row,
                folded,
            } => {
                if layout.latest == Some(*exchange) {
                    frame.buffer_mut().set_style(row_area, emphasis.primary);
                } else {
                    older_answer = true;
                }
                let answers = if *folded {
                    &state.presentation.short_answers
                } else {
                    &state.presentation.answers
                };
                &answers[*exchange].as_ref().expect("answer was populated")[*row]
            }
            TranscriptRow::Display(line) => line,
        };
        if selected == Some(view.scroll + offset) && (view.history_visible || !view.follow_tail) {
            frame.render_widget(
                ratatui::widgets::Block::default().style(styles::selected_style(theme)),
                row_area,
            );
        }
        frame.render_widget(row, row_area);
        if older_answer {
            frame.buffer_mut().set_style(row_area, emphasis.secondary);
        }
    }
    if gutter > 0 && view.max_scroll > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .track_style(emphasis.muted)
            .thumb_symbol("┃")
            .thumb_style(emphasis.secondary);
        let mut scroll = ScrollbarState::new(layout.rows.len())
            .position(view.scroll)
            .viewport_content_length(view.page_height);
        frame.render_stateful_widget(scrollbar, area, &mut scroll);
    }
}

fn build_transcript(
    state: &mut CompanionState,
    theme: &Theme,
    width: usize,
    stale: bool,
    density: TranscriptDensity,
) -> Transcript {
    let mut layout = Transcript {
        width,
        density,
        latest: None,
        selected_detail: None,
        rows: Vec::new(),
        selected: None,
    };
    let Some(view) = state.view.as_ref() else {
        return layout;
    };
    let emphasis = styles::review_emphasis(theme);
    if view.history_visible {
        for turn in &state.history {
            let question = turn.question.lines().next().unwrap_or_default();
            let earlier = if view.is_history_earlier(&turn.anchor) {
                " · earlier version"
            } else {
                ""
            };
            let row = format!("{}{}  —  {question}", turn.anchor.label(), earlier);
            // One row per result so ↑/↓ and Enter never land inside a wrapped answer.
            layout.single_row(row, emphasis.secondary);
        }
        if layout.rows.is_empty() {
            layout.push(Line::styled(
                "No private exchanges for this review yet.",
                emphasis.muted,
            ));
        }
        return layout;
    }

    let draft = (!stale).then_some(view.feedback.as_deref()).flatten();
    if state.streaming_turn().is_none() && draft.is_none() {
        layout.latest = state
            .history
            .iter()
            .rposition(|turn| view.matches_thread(&turn.anchor));
    }
    let prepared = (!stale).then(|| view.guide_preview()).flatten();
    if let Some((explanation, followups)) = prepared {
        // Preserve the prepared questions before spending scarce rows on
        // explanatory prose. The full explanation comes back as the dock grows.
        if matches!(
            density,
            TranscriptDensity::Full | TranscriptDensity::Compact
        ) || (density == TranscriptDensity::Tight && followups.is_empty())
        {
            layout.rule("Prepared explanation", emphasis.muted);
            layout.indented_markdown(
                explanation,
                theme,
                emphasis.secondary,
                1,
                density != TranscriptDensity::Full,
            );
        }
        if !followups.is_empty() && density != TranscriptDensity::Bare {
            layout.rule("Prepared questions", emphasis.muted);
        }
        for (index, followup) in followups.iter().enumerate() {
            let selected = view.followup_selected == Some(index);
            let marker = if selected {
                layout.selected = Some(layout.rows.len());
                "›"
            } else {
                "·"
            };
            let tone = if selected {
                emphasis.primary
            } else {
                emphasis.secondary
            };
            layout.single_row(
                format!("{marker} {}", safe_display(&followup.question)),
                tone,
            );
            if let Some((exchange, turn)) =
                state.history.iter().enumerate().rev().find(|(_, turn)| {
                    turn.provider == GUIDE_PROVIDER
                        && turn.question == followup.question
                        && view.matches_thread(&turn.anchor)
                        && !view.is_earlier(&turn.anchor)
                })
            {
                layout.push(Line::styled("  Prepared answer", emphasis.muted));
                let tone = if layout.latest == Some(exchange) {
                    emphasis.primary
                } else {
                    emphasis.secondary
                };
                let first = layout.rows.len();
                layout.indented_markdown(&turn.answer, theme, tone, 2, false);
                if selected && view.focus_prepared_answer && layout.rows.len() > first {
                    layout.selected_detail = Some(first);
                }
            }
        }
    }
    let mut conversation_started = false;
    state
        .presentation
        .answers
        .resize_with(state.history.len(), || None);
    state
        .presentation
        .short_answers
        .resize_with(state.history.len(), || None);
    for (exchange, turn) in state.history.iter().enumerate() {
        if !view.matches_thread(&turn.anchor) {
            continue;
        }
        let prepared_answer = turn.provider == GUIDE_PROVIDER;
        if prepared_answer
            && !view.is_earlier(&turn.anchor)
            && prepared.is_some_and(|(_, followups)| {
                followups.iter().any(|item| item.question == turn.question)
            })
        {
            continue;
        }
        layout.start_turn(&mut conversation_started, emphasis.muted);
        layout.question(&turn.question, emphasis.muted, emphasis.secondary);
        let earlier = if view.is_earlier(&turn.anchor) || stale {
            " · earlier version"
        } else {
            ""
        };
        if prepared_answer {
            layout.push(Line::styled(
                format!("  Prepared answer{earlier}"),
                emphasis.muted,
            ));
            let tone = if layout.latest == Some(exchange) {
                emphasis.primary
            } else {
                emphasis.secondary
            };
            layout.indented_markdown(&turn.answer, theme, tone, 2, false);
            continue;
        }
        layout.push(Line::styled(
            format!("{}{earlier}", safe_display(&turn.provider)),
            emphasis.muted,
        ));
        let short = if view.shows_full_answer(turn) {
            None
        } else {
            turn.short_preview()
        };
        let (cache, text) = if let Some(short) = short {
            (&mut state.presentation.short_answers, short)
        } else {
            (&mut state.presentation.answers, turn.answer.as_str())
        };
        let answer = cache[exchange].get_or_insert_with(|| markdown::render(text, theme));
        for (row, line) in answer.iter().enumerate() {
            if line.width() <= width {
                layout.rows.push(TranscriptRow::Answer {
                    exchange,
                    row,
                    folded: short.is_some(),
                });
            } else if layout.latest == Some(exchange) {
                layout.wrap(line, emphasis.primary, false);
            } else {
                layout.wrap(line, emphasis.secondary, true);
            }
        }
    }
    if let Some(draft) = draft {
        layout.start_turn(&mut conversation_started, emphasis.muted);
        layout.push(Line::styled("Feedback draft", emphasis.muted));
        layout.markdown(draft, theme, emphasis.primary);
    }
    if let Some((question, text, feedback)) = state.streaming_turn() {
        layout.start_turn(&mut conversation_started, emphasis.muted);
        layout.question(question, emphasis.muted, emphasis.secondary);
        if !text.is_empty() {
            let label = if feedback { "Drafting" } else { "Streaming" };
            layout.push(Line::styled(label, emphasis.muted));
            layout.markdown(text, theme, emphasis.primary);
        }
    }
    if layout.rows.is_empty() {
        layout.push(Line::styled(
            "Sending shares reviewed source.",
            emphasis.muted,
        ));
    }
    layout
}

impl Transcript {
    fn question(&mut self, question: &str, label: Style, text: Style) {
        self.push(Line::styled("You", label));
        for line in question.split('\n') {
            self.push(Line::styled(safe_display(line).into_owned(), text));
        }
    }

    fn start_turn(&mut self, started: &mut bool, muted: Style) {
        if *started {
            self.push(Line::default());
        } else if !self.rows.is_empty() {
            self.rule("", muted);
        }
        *started = true;
    }

    fn rule(&mut self, title: &str, style: Style) {
        let rest = self.width.saturating_sub(title.width() + 3);
        self.single_row(
            if title.is_empty() {
                "─".repeat(self.width)
            } else {
                format!("─ {title} {}", "─".repeat(rest))
            },
            style,
        );
    }

    fn single_row(&mut self, text: String, style: Style) {
        let end = prefix_within_width(&text, self.width).len();
        let mut text = text;
        text.truncate(end);
        self.rows
            .push(TranscriptRow::Display(Line::styled(text, style)));
    }

    fn indented_markdown(
        &mut self,
        text: &str,
        theme: &Theme,
        tone: Style,
        margin: usize,
        compact: bool,
    ) {
        let columns = self.width.saturating_sub(margin);
        if columns == 0 {
            return;
        }
        for line in markdown::render(text, theme) {
            if compact {
                let mut preview = " ".repeat(margin);
                let mut remaining = columns.saturating_sub(1);
                for span in &line.spans {
                    let prefix = prefix_within_width(span.content.as_ref(), remaining);
                    preview.push_str(prefix);
                    remaining = remaining.saturating_sub(prefix.width());
                    if prefix.len() < span.content.len() || remaining == 0 {
                        break;
                    }
                }
                preview.push('…');
                self.single_row(preview, tone);
                return;
            }
            for mut spans in wrap_spans(&line.spans, columns) {
                for span in &mut spans {
                    span.style = span.style.patch(tone);
                }
                spans.insert(0, Span::raw(" ".repeat(margin)));
                self.rows.push(TranscriptRow::Display(Line::from(spans)));
            }
        }
    }

    fn markdown(&mut self, text: &str, theme: &Theme, tone: Style) {
        for mut row in markdown::render(text, theme) {
            row.style = row.style.patch(tone);
            self.push(row);
        }
    }

    fn push(&mut self, row: Line<'static>) {
        if row.width() <= self.width {
            self.rows.push(TranscriptRow::Display(row));
        } else {
            self.wrap(&row, Style::default(), false);
        }
    }

    fn wrap(&mut self, row: &Line<'static>, tone: Style, flatten_ink: bool) {
        self.rows.extend(
            wrap_spans(&row.spans, self.width)
                .into_iter()
                .map(|mut spans| {
                    if flatten_ink {
                        for span in &mut spans {
                            span.style = span.style.patch(tone);
                        }
                    }
                    TranscriptRow::Display(Line::from(spans).style(row.style.patch(tone)))
                }),
        );
    }
}

pub(super) fn preview_rows(
    preview: CompanionPreview<'_>,
    theme: &Theme,
    width: usize,
    row_limit: usize,
) -> Vec<Line<'static>> {
    if row_limit == 0 {
        return Vec::new();
    }
    let mut source = preview.text.lines();
    let mut lines = source
        .by_ref()
        .take(row_limit)
        .map(safe_display)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push("".into());
    }
    let truncated = preview.truncated || source.next().is_some();
    let last = lines.len() - 1;
    let mut highlights = preview.source_path.and_then(|path| {
        theme
            .syntax_highlighter()
            .highlight_source_excerpt(path, &lines)
            .map(Vec::into_iter)
    });
    lines
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            let end = truncated && idx == last;
            let fit = prefix_within_width(&text, width);
            let marker = if end && width >= 30 {
                " … (truncated)"
            } else if end || fit.len() < text.len() {
                "…"
            } else {
                ""
            };
            let mut limit = if marker.is_empty() {
                fit.len()
            } else {
                prefix_within_width(fit, width.saturating_sub(marker.width())).len()
            };
            let runs = highlights.as_mut().and_then(Iterator::next).flatten();
            let mut spans = if let Some(runs) = runs {
                syntax_spans(runs, theme.fg_primary, theme.bg_highlight)
                    .map_while(|mut span| {
                        if limit == 0 {
                            return None;
                        }
                        if span.content.len() > limit {
                            span.content.to_mut().truncate(limit);
                        }
                        limit -= span.content.len();
                        Some(span)
                    })
                    .collect()
            } else {
                vec![Span::raw(text[..limit].to_owned())]
            };
            if !marker.is_empty() {
                spans.push(Span::raw(marker));
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn companion_transcript_wraps_styled_rows_by_display_cells() {
        let mut layout = Transcript {
            width: 3,
            density: TranscriptDensity::Full,
            latest: None,
            selected_detail: None,
            rows: Vec::new(),
            selected: None,
        };
        for row in markdown::render("**a👩‍💻界**", &Theme::dark()) {
            layout.push(row);
        }
        assert_eq!(layout.rows.len(), 2);
        for (row, expected) in layout.rows.iter().zip(["a👩‍💻", "界"]) {
            let TranscriptRow::Display(row) = row else {
                panic!("wrapped text should be a display row");
            };
            assert_eq!(row.spans[0].content, expected);
            assert!(row.spans[0].style.add_modifier.contains(Modifier::BOLD));
            assert!(row.width() <= layout.width);
        }
    }
}
