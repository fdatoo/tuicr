use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{App, coverage::VerdictCounts};
use crate::ui::{styles, text_utils::wrap_spans};

pub(super) fn render(frame: &mut Frame, app: &mut App) {
    let theme = &app.theme;
    let block = Block::default()
        .title(" Coverage ")
        .borders(Borders::ALL)
        .style(styles::panel_style(theme))
        .border_style(styles::border_style(theme, true));
    let inner = block.inner(frame.area());
    frame.render_widget(block, frame.area());
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let mut lines = Vec::new();
    let heading = Style::default().add_modifier(Modifier::BOLD);
    if let Some(record) = app.coverage_record() {
        lines.push(Span::raw(format!("Review guide {}", record.reach_label())));
        for row in record.compact_summary() {
            lines.push(Span::raw(row));
        }
        lines.push(Span::raw(format!(
            "Review marks: {} files · {} hunks    Active time: {}m {:02}s",
            record.file_marks,
            record.hunk_marks,
            record.elapsed_secs / 60,
            record.elapsed_secs % 60,
        )));
        if record.stale {
            lines.push(Span::styled(
                "Guide is for an earlier diff; only unchanged stops retain verdicts.",
                Style::default().fg(theme.message_warning_fg),
            ));
        }
        lines.push(Span::raw(""));
        lines.push(Span::styled(
            format!("Stories · {}", counts(&record.story_totals)),
            heading,
        ));
        lines.push(Span::styled(
            format!("Stops · {}", counts(&record.stops)),
            heading,
        ));
        for (index, story) in record.stories.iter().enumerate() {
            lines.push(Span::raw(format!(
                "  {}. {} · {}",
                index + 1,
                story.title,
                counts(&story.stops)
            )));
        }
        lines.push(Span::raw(""));
        lines.push(Span::styled("Supporting changes", heading));
        for group in record
            .groups
            .iter()
            .chain((record.unassigned.total > 0).then_some(&record.unassigned))
        {
            let name = if group.label.is_empty() {
                "Not in the guide"
            } else {
                group.label
            };
            let state = if group.dismissed {
                " · mechanical dismissal"
            } else if group.spot_verdicts == 0 && group.unreviewed() == group.total {
                " · untouched"
            } else {
                ""
            };
            lines.push(Span::raw(format!(
                "  {name} · {} hunks: {} verified, {} dismissed, {} unverified{state}",
                group.total,
                group.reviewed,
                group.dismissed_hunks,
                group.unreviewed()
            )));
        }
        if record.groups.is_empty() && record.unassigned.total == 0 {
            lines.push(Span::raw("  No supporting hunks"));
        }
        let spot = &record.spot_checks;
        lines.push(Span::raw(format!(
            "  Spot-check verdicts: {} recorded · {} checked out, {} wrong, {} parked",
            spot.decided(),
            spot.verified,
            spot.wrong,
            spot.parked
        )));
    } else {
        lines.push(Span::raw("No review guide for the current diff."));
    }
    let wrapped: Vec<Line> = lines
        .into_iter()
        .flat_map(|line| {
            if line.content.is_empty() {
                vec![Line::default()]
            } else {
                wrap_spans(&[line], usize::from(body.width.max(1)))
                    .into_iter()
                    .map(Line::from)
                    .collect()
            }
        })
        .collect();
    let view = &mut app.coverage_view;
    view.max_scroll = wrapped.len().saturating_sub(usize::from(body.height));
    view.scroll = view.scroll.min(view.max_scroll);
    let rows: Vec<_> = wrapped
        .into_iter()
        .skip(view.scroll)
        .take(usize::from(body.height))
        .collect();
    frame.render_widget(Paragraph::new(rows).style(styles::panel_style(theme)), body);
    frame.render_widget(
        Paragraph::new("j/k scroll · Esc close").style(styles::dim_style(theme)),
        footer,
    );
}

fn counts(counts: &VerdictCounts) -> String {
    format!(
        "{} verified, {} wrong, {} parked, {} unchecked",
        counts.verified, counts.wrong, counts.parked, counts.unchecked
    )
}
