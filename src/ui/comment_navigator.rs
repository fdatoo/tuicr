use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, CommentNavigatorItem, CommentNavigatorKind, FocusedPanel};
use crate::ui::diff_view::apply_horizontal_scroll;
use crate::ui::styles;

pub(super) fn render_comment_navigator(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focused_panel == FocusedPanel::Comments;
    let count = app.comment_navigator_items().len();
    let title = format!(" Comments · {count} ");
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(styles::panel_style(&app.theme))
        .border_style(styles::border_style(&app.theme, focused));

    let inner = block.inner(area);
    app.comment_navigator_inner_area = Some(inner);
    app.comment_navigator_state.viewport_width = inner.width as usize;
    app.comment_navigator_state.viewport_height = inner.height as usize;
    app.sync_comment_navigator_selection();

    let max_content_width = app.comment_navigator_max_width(|item| comment_row_width(app, item));
    app.comment_navigator_state.max_content_width = max_content_width;

    let max_scroll_x = max_content_width.saturating_sub(inner.width as usize);
    if app.comment_navigator_state.scroll_x > max_scroll_x {
        app.comment_navigator_state.scroll_x = max_scroll_x;
    }
    let scroll_x = app.comment_navigator_state.scroll_x;

    if inner.is_empty() || count == 0 {
        frame.render_widget(block, area);
        return;
    }
    let state = &mut app.comment_navigator_state.list_state;
    let selected = state.selected();
    let height = usize::from(inner.height);
    let offset = visible_offset(state.offset(), selected, count, height);
    *state.offset_mut() = offset;
    let rows = app.comment_navigator_items()[offset..]
        .iter()
        .take(height)
        .map(|item| {
            ListItem::new(apply_horizontal_scroll(
                render_comment_row(app, item),
                scroll_x,
            ))
        });

    let list = List::new(rows)
        .style(styles::panel_style(&app.theme))
        .highlight_style(styles::selected_style(&app.theme))
        .block(block);

    let mut visible = ListState::default().with_selected(selected.map(|index| index - offset));
    frame.render_stateful_widget(list, area, &mut visible);
}

// As with a full ratatui List, keep the offset unless the selected one-line
// item would be offscreen. Preserve absolute indices for mouse and keyboard.
fn visible_offset(offset: usize, selected: Option<usize>, count: usize, height: usize) -> usize {
    let offset = offset.min(count - 1);
    selected.map_or(offset, |index| {
        offset.clamp(index.saturating_sub(height - 1), index)
    })
}

fn comment_marker(app: &App, item: &CommentNavigatorItem) -> char {
    match &item.kind {
        // `None` comments have an empty label; show a neutral bullet marker.
        CommentNavigatorKind::Local(comment_type) => app
            .comment_type_label(comment_type)
            .chars()
            .next()
            .unwrap_or('•'),
        CommentNavigatorKind::Remote { .. } => 'R',
    }
}

fn comment_row_width(app: &App, item: &CommentNavigatorItem) -> usize {
    let marker_width = comment_marker(app, item).width().unwrap_or(0) + 1;
    let author_width = if item
        .author
        .as_deref()
        .is_some_and(|author| author != app.username)
    {
        "● ".width()
    } else {
        0
    };
    let location_width = match item.path.as_deref() {
        None => "review".len(),
        Some(path) => {
            path.width()
                + item.line.map_or(0, |line| {
                    // One colon followed by the decimal line number.
                    2 + line.checked_ilog10().unwrap_or(0) as usize
                })
        }
    };
    marker_width + author_width + location_width
}

fn render_comment_row(app: &App, item: &CommentNavigatorItem) -> Line<'static> {
    let marker = comment_marker(app, item).to_string();
    let marker_style = match &item.kind {
        CommentNavigatorKind::Local(comment_type) => {
            styles::comment_type_style(&app.theme, app.comment_type_color(comment_type))
        }
        CommentNavigatorKind::Remote { muted: true } => styles::dim_style(&app.theme),
        CommentNavigatorKind::Remote { muted: false } => Style::default()
            .fg(app.theme.diff_hunk_header)
            .add_modifier(Modifier::BOLD),
    };

    let dim_style = styles::dim_style(&app.theme);
    let location = comment_location(item);

    let author_accent = item
        .author
        .as_deref()
        .and_then(|author| styles::author_accent(&app.username, author));

    let mut spans = vec![Span::styled(marker, marker_style), Span::raw(" ")];
    if let Some(color) = author_accent {
        spans.push(Span::styled(
            "● ".to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(location, dim_style));
    Line::from(spans)
}

fn comment_location(item: &CommentNavigatorItem) -> String {
    let Some(path) = item.path.as_deref() else {
        return "review".to_string();
    };

    let mut location = path.to_string();
    if let Some(line) = item.line {
        location.push(':');
        location.push_str(&line.to_string());
    }
    location
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn clipped_comment_navigator_preserves_full_list_offset() {
        let mut terminal = Terminal::new(TestBackend::new(10, 4)).unwrap();
        for (offset, selected) in [(5, 6), (5, 11), (8, 2), (99, 11), (10, 11)] {
            let mut full = ListState::default()
                .with_offset(offset)
                .with_selected(Some(selected));
            terminal
                .draw(|frame| {
                    frame.render_stateful_widget(List::new(["item"; 12]), frame.area(), &mut full);
                })
                .unwrap();
            assert_eq!(visible_offset(offset, Some(selected), 12, 4), full.offset());
        }
    }
}
