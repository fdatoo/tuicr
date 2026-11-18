use std::collections::HashMap;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, DiffSource, DiffViewMode, FocusedPanel, GuideEntryCue, InputMode, MapEntry, MapRow,
};
use crate::review_guide::{Audit, Basis, RiskKind, SITTING_MINUTES, Side, Stop, Verdict};
use crate::ui::{
    styles,
    text_utils::{prefix_within_width, truncate_str, wrap_spans},
};

pub(super) fn render_map(frame: &mut Frame, app: &mut App, area: Rect) {
    let (rows_area, hits) = draw_map(frame, app, area);
    app.set_guide_map_viewport(Some(rows_area), hits);
}

fn draw_map(frame: &mut Frame, app: &App, area: Rect) -> (Rect, Vec<Option<MapEntry>>) {
    let (Some(guide), Some(progress)) = (app.active_guide(), app.guide_progress()) else {
        return (Rect::default(), Vec::new());
    };
    let focused = app.focused_panel == FocusedPanel::FileList;
    let tone = styles::review_emphasis(&app.theme);
    let title = if app.showing_changes_only() {
        " Changes "
    } else {
        " Map "
    };
    let border = Block::default()
        .borders(Borders::ALL)
        .title(Line::styled(title, tone.secondary))
        .border_style(styles::border_style(&app.theme, focused))
        .style(styles::panel_style(&app.theme));
    let inner = border.inner(area);
    frame.render_widget(border, area);
    if inner.is_empty() {
        return (inner, Vec::new());
    }

    let dim = tone.muted;
    let accent = styles::current_line_indicator_style(&app.theme);
    let warning = styles::review_warning(&app.theme);
    let mut checked = None;
    let wrong = Style::default().fg(app.theme.comment_issue);
    let mut content = inner;
    let footer = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    content.height -= 1;
    let count = guide.stop_count();
    let stop_label = if count == 1 { "stop" } else { "stops" };
    let estimate = guide.estimate_minutes.map_or_else(
        || format!("{count} {stop_label} · no estimate"),
        |minutes| format!("{count} {stop_label} · ~{minutes} min"),
    );
    if let Some((covered, total)) = app.guide_reach() {
        let reach = format!("{covered} of {total} hunks");
        let summary = format!("{estimate} · {reach}");
        if summary.width() <= usize::from(inner.width) {
            draw_line(frame, take_top(&mut content, 1), &summary, dim);
        } else {
            draw_line(frame, take_top(&mut content, 1), &estimate, dim);
            draw_line(frame, take_top(&mut content, 1), &reach, dim);
        }
    } else {
        draw_line(frame, take_top(&mut content, 1), &estimate, dim);
    }
    let left = guide
        .stops()
        .filter(|(_, stop)| !progress.verdicts.contains_key(&stop.id))
        .count();
    if let Some((stop, stops, story, stories)) = app.guide_stop_numbers() {
        let here = format!("stop {stop} of {stops}");
        let larger = format!("story {story} of {stories}");
        let progress = format!("{here} · {larger} · {left} left");
        let ordinals = format!("{here} · {larger}");
        let width = usize::from(inner.width);
        if progress.width() <= width {
            draw_line(frame, take_top(&mut content, 1), &progress, dim);
        } else if ordinals.width() <= width {
            draw_line(frame, take_top(&mut content, 1), &ordinals, dim);
            draw_line(
                frame,
                take_top(&mut content, 1),
                &format!("{left} left"),
                dim,
            );
        } else {
            draw_line(frame, take_top(&mut content, 1), &here, dim);
            draw_line(
                frame,
                take_top(&mut content, 1),
                &format!("{larger} · {left} left"),
                dim,
            );
        }
    } else {
        let label = if left == 0 {
            "Stories finished"
        } else {
            "No visible stops left"
        };
        draw_line(frame, take_top(&mut content, 1), label, dim);
    }
    if !app.showing_changes_only() && !guide.provenance.audited {
        draw_line(frame, take_top(&mut content, 1), "Not audited", dim);
    }
    if app.installed_guide_is_stale() {
        let label = if inner.width >= 26 {
            "Earlier version · :prepare"
        } else {
            "Stale · :prepare"
        };
        draw_line(frame, take_top(&mut content, 1), label, warning);
    }
    if app.installed_guide_has_older_rules() {
        draw_line(frame, take_top(&mut content, 1), "Rules changed", warning);
    }
    if left == count
        && guide
            .estimate_minutes
            .is_some_and(|minutes| minutes > SITTING_MINUTES)
    {
        let full = "Long: now · approve verified · split";
        let compact = "Long: now/check/split";
        let label = if full.width() <= usize::from(inner.width) {
            full
        } else {
            compact
        };
        draw_line(frame, take_top(&mut content, 1), label, dim);
    }
    if content.height >= 4 {
        let rule = take_top(&mut content, 1);
        frame.render_widget(horizontal_rule(app, rule.width), rule);
    }
    let supporting_open = app.guide_supporting_open();
    let footer_text = if focused
        && supporting_open
        && guide.supporting.iter().any(|group| {
            group.basis == Basis::Mechanical && !progress.dismissed_groups.contains(&group.label)
        }) {
        format!("D dismiss · {} min", progress.elapsed_secs / 60)
    } else {
        format!("{} min active", progress.elapsed_secs / 60)
    };
    draw_line(frame, footer, &footer_text, dim);

    // Preserve one line for the collapsed pile rather than making it scroll
    // out of sight while there are still undecided story stops.
    if !app.showing_changes_only() && !supporting_open && content.height > 1 {
        content.height -= 1;
        let pile = Rect::new(content.x, content.bottom(), content.width, 1);
        draw_line(frame, pile, "Supporting changes ▸", dim);
        if content.height >= 3 {
            content.height -= 1;
            let rule = Rect::new(content.x, content.bottom(), content.width, 1);
            frame.render_widget(horizontal_rule(app, rule.width), rule);
        }
    }
    if content.is_empty() {
        return (content, Vec::new());
    }
    let (selected, current) = app.guide_map_selection();
    let target = if focused {
        selected.or(current)
    } else {
        current.or(selected)
    };
    let roomy = content.height >= 5;
    let mut rows = Vec::new();
    for row in app.guide_map_rows() {
        let entry = row.entry();
        let is_current = entry.is_some() && entry == current;
        let mut style = if is_current {
            tone.primary.add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        if entry.is_some() && entry == selected && focused {
            style = style.bg(app.theme.bg_highlight);
        }
        match row {
            MapRow::Story {
                title,
                current,
                total,
                collapsed,
            } => {
                if roomy {
                    rows.push((None, Line::default()));
                }
                let mut heading = if current {
                    tone.primary.add_modifier(Modifier::BOLD)
                } else {
                    tone.secondary
                };
                heading.bg = style.bg;
                if collapsed.is_some() {
                    let count = format!(" ✓ {total}/{total}");
                    let text_width = content.width.saturating_sub(count.width() as u16);
                    let mut line = short_line(title, text_width, heading);
                    line.spans.push(Span::styled(count, dim));
                    rows.push((entry, line));
                } else {
                    for line in clipped_prose(title, content.width, if roomy { 2 } else { 1 }, "…")
                    {
                        rows.push((None, line.style(heading)));
                    }
                }
            }
            MapRow::Notice(text) => rows.push((None, short_line(text, content.width, dim))),
            MapRow::Stop { index, stop } => {
                let verdict = progress.verdicts.get(&stop.id).map(|record| record.verdict);
                let changed = app.guide_stop_changed(index);
                let mark = match verdict {
                    _ if changed => ("↻", accent),
                    Some(Verdict::ChecksOut) => (
                        "✓",
                        *checked.get_or_insert_with(|| {
                            let panel = app.theme.palette_panel_bg();
                            Style::default().fg(styles::guide_surround_colors(
                                app.theme.reviewed,
                                panel,
                                panel,
                            )
                            .0)
                        }),
                    ),
                    Some(Verdict::Wrong) => ("×", wrong),
                    Some(Verdict::Parked) => ("p", warning),
                    None => ("·", dim),
                };
                if !is_current && verdict.is_none() && !changed {
                    style.fg = tone.secondary.fg;
                }
                // The mark asks for attention, so it goes once the reviewer has
                // decided; a parked stop still needs that decision.
                let risk = stop.needs_explicit_verdict()
                    && matches!(verdict, None | Some(Verdict::Parked));
                let label = if changed { "Changed" } else { &stop.claim };
                let height = if is_current { 2 } else { 1 };
                for (i, mut line) in
                    clipped_prose(label, content.width.saturating_sub(5), height, "…").enumerate()
                {
                    if i == 0 {
                        let marker = if is_current { "▏" } else { " " };
                        let risk_mark = if risk { "!" } else { " " };
                        line.spans.splice(
                            0..0,
                            [
                                Span::styled(marker, accent),
                                Span::raw(" "),
                                Span::styled(mark.0, mark.1),
                                Span::styled(risk_mark, warning),
                                Span::raw(" "),
                            ],
                        );
                    } else {
                        line.spans.insert(0, Span::raw("     "));
                    }
                    rows.push((entry, line.style(style)));
                }
            }
            MapRow::Supporting if supporting_open => {
                if roomy && !rows.is_empty() {
                    rows.push((None, horizontal_rule(app, content.width)));
                }
                rows.push((None, Line::styled("Supporting changes", dim)));
            }
            MapRow::Supporting => {}
            MapRow::Group(index) => {
                let group = &guide.supporting[index];
                let label = if progress.dismissed_groups.contains(&group.label) {
                    format!(" ✓ {}", group.label)
                } else if group.basis == Basis::Mechanical {
                    format!(" D {} ({})", group.label, group.hunks.len())
                } else {
                    format!(" ? {} ({})", group.label, group.hunks.len())
                };
                rows.push((entry, short_line(&label, content.width, style)));
            }
        }
    }
    let target_row =
        target.and_then(|target| rows.iter().position(|(entry, _)| *entry == Some(target)));
    let height = usize::from(content.height);
    let offset = target_row
        .unwrap_or(0)
        .saturating_sub(height / 2)
        .min(rows.len().saturating_sub(height));
    let (hits, lines): (Vec<_>, Vec<_>) = rows.into_iter().skip(offset).take(height).unzip();
    frame.render_widget(Paragraph::new(lines), content);
    (content, hits)
}

pub(super) fn render_stop_card(
    frame: &mut Frame,
    app: &App,
    mut area: Rect,
    essential_limit: u16,
    optional_limit: u16,
) -> Rect {
    if let Some((position, total)) = app.spot_position()
        && area.height > 4
    {
        let label = format!("spot-check {position} of {total}");
        let [banner, rest] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(4),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(format!(
                "{label}  ·  . checks out  x wrong  p park  Esc stop"
            ))
            .style(styles::panel_style(&app.theme)),
            banner,
        );
        return rest;
    }
    if app.active_guide().is_none() {
        return render_entry_cue(frame, app, area);
    }
    if area.width == 0 || essential_limit == 0 {
        return area;
    }
    if let Some(stop) = app.current_stop() {
        render_stop(frame, app, &mut area, stop, essential_limit, optional_limit);
        return area;
    }
    let height = essential_limit.min(4);
    let tone = styles::review_emphasis(&app.theme);
    let used;
    let mut body = Rect::new(area.x, area.y, area.width, height);
    if let Some(progress) = app.guide_progress() {
        let undecided = app
            .guide_resolved_stops()
            .filter(|(stop, _)| !progress.verdicts.contains_key(&stop.id))
            .count();
        let parked = progress
            .verdicts
            .values()
            .filter(|record| record.verdict == Verdict::Parked)
            .count();
        let supporting = app
            .coverage_record()
            .map_or(0, |record| record.supporting_unreviewed());
        let hunk = if supporting == 1 { "hunk" } else { "hunks" };
        let heading = if undecided == 0 {
            "Stories finished"
        } else {
            "No visible unchecked stops"
        };
        draw_line(frame, take_top(&mut body, 1), heading, tone.secondary);
        let remaining = if undecided == 0 {
            format!("{parked} parked · {supporting} supporting {hunk} left")
        } else {
            format!(
                "{undecided} hidden or changed · {parked} parked · {supporting} supporting {hunk} left"
            )
        };
        let remaining_limit = body.height.saturating_sub(1).min(2);
        draw_wrapped(
            frame,
            &mut body,
            &remaining,
            tone.secondary,
            remaining_limit,
        );
        let keys = if app.showing_changes_only() {
            ":changes  , prev  t map"
        } else if matches!(app.diff_source, DiffSource::PullRequest(_)) {
            ":spot  :coverage  :submit  , prev  t map"
        } else {
            ":spot  :coverage  , prev  t map"
        };
        draw_line(frame, take_top(&mut body, 1), keys, tone.muted);
        used = body.y - area.y;
    } else {
        used = 0;
    }
    Rect::new(area.x, area.y + used, area.width, area.height - used)
}

fn render_stop(
    frame: &mut Frame,
    app: &App,
    area: &mut Rect,
    stop: &Stop,
    essential_limit: u16,
    optional_limit: u16,
) {
    let tone = styles::review_emphasis(&app.theme);
    let title = tone.primary.add_modifier(Modifier::BOLD);
    let warning = styles::review_warning(&app.theme);
    let inset = u16::from(area.width >= 65);
    let panel = area;
    let mut text_area = Rect::new(
        panel.x + inset,
        panel.y,
        panel.width - 2 * inset,
        panel.height,
    );
    let area = &mut text_area;
    let notices = stop_notices(app, stop);
    let width = area.width;
    let essential_limit = essential_limit.min(area.height);
    let quarter = area.height.div_ceil(4).max(5).min(essential_limit);
    let claim = prose(&stop.claim, width);
    let claim_rows = claim.len().min(3) as u16;
    let notices: Vec<_> = notices.iter().map(|notice| prose(notice, width)).collect();
    let evidence = notices.iter().map(Vec::len).sum::<usize>();
    let essential = usize::from(claim_rows) + evidence;

    // On short screens Ask owns the keys; keep the claim and notices above
    // the code but give its composer and prepared questions the key rows.
    let guide_keys = !(app.input_mode == InputMode::Companion && width <= 80 && area.height <= 16);
    let explicit = stop.needs_explicit_verdict();
    let (mut keys, mut second_keys) = stop_keys(explicit, width, essential_limit);
    if second_keys.is_some() && essential + 2 > usize::from(essential_limit) {
        (keys, second_keys) = stop_keys(explicit, width, 1);
    }
    let key_rows = if guide_keys {
        1 + u16::from(second_keys.is_some())
    } else {
        0
    };
    let body_limit = quarter
        .max(
            u16::try_from(essential)
                .unwrap_or(u16::MAX)
                .saturating_add(key_rows),
        )
        .min(essential_limit)
        .saturating_sub(key_rows);
    let claim_limit = body_limit
        .saturating_sub(u16::from(body_limit > 1 && !notices.is_empty()))
        .min(claim_rows);
    draw_prose(frame, area, claim, title, claim_limit, "…");

    // Optional prose must fit alongside the complete evidence, not displace it.
    let mut spare = quarter.min(optional_limit).saturating_sub(
        u16::try_from(essential)
            .unwrap_or(u16::MAX)
            .saturating_add(key_rows),
    );
    if spare > 0 {
        draw_line(
            frame,
            take_top(area, 1),
            &stop_location(app, stop),
            tone.muted,
        );
        spare -= 1;
    }
    let pad_before_diff = spare > 0 && (spare >= 3 || stop.explanation.is_empty());
    spare -= u16::from(pad_before_diff);
    let mut draw_key_rule = spare > 0;
    if !stop.explanation.is_empty() && spare > 0 {
        if spare >= 3 {
            take_top(area, 1);
            spare -= 1;
        }
        let explanation = prose(&stop.explanation, width);
        draw_key_rule = spare > (explanation.len().min(2) as u16);
        draw_prose(frame, area, explanation, tone.secondary, spare.min(2), "…");
    }

    let mut remaining = body_limit.saturating_sub(claim_limit);
    let notice_count = notices.len();
    for (i, lines) in notices.into_iter().enumerate() {
        let reserve = (notice_count - i - 1).min(usize::from(remaining.saturating_sub(1))) as u16;
        let limit = remaining.saturating_sub(reserve);
        let height = lines.len().min(usize::from(limit)) as u16;
        draw_prose(frame, area, lines, warning, height, "…");
        remaining -= height;
    }
    if guide_keys && draw_key_rule {
        frame.render_widget(horizontal_rule(app, width.min(24)), take_top(area, 1));
    }

    if guide_keys {
        draw_stop_keys(frame, take_top(area, 1), keys, &tone);
        if let Some(second) = second_keys {
            draw_stop_keys(frame, take_top(area, 1), second, &tone);
        }
    }
    if pad_before_diff {
        let boundary = take_top(area, 1);
        if app.is_diff_sole_pane() {
            frame.render_widget(horizontal_rule(app, width), boundary);
        }
    }
    take_top(panel, text_area.y - panel.y);
}

fn stop_location(app: &App, stop: &Stop) -> String {
    let (start, end) = app
        .guide_stop_range()
        .unwrap_or((stop.start_line, stop.end_line));
    let location = if start == end {
        format!("{}:{start}", stop.path)
    } else {
        format!("{}:{start}–{end}", stop.path)
    };
    let side = if stop.side == Side::Old { " (old)" } else { "" };
    match &stop.symbol {
        Some(symbol) if !symbol.is_empty() => format!("{symbol} · {location}{side}"),
        _ => format!("{location}{side}"),
    }
}

fn stop_notices(app: &App, stop: &Stop) -> Vec<String> {
    let mut status = Vec::new();
    if let Some(risk) = &stop.risk {
        status.push(format!("Risk item: {}", risk.reason));
    }
    if stop.untested {
        status.push("Untested: no covering test found".to_string());
    }
    if let Audit::Doubtful { note } = &stop.audit {
        status.push(format!("Doubt: {note}"));
    }
    if let Some(test) = &stop.test {
        let failed = app.pr_info.as_ref().is_some_and(|info| {
            info.checks.iter().any(|check| {
                !test.name.is_empty()
                    && check.name == test.name
                    && matches!(
                        check.conclusion.as_deref(),
                        Some("FAILURE" | "ERROR" | "TIMED_OUT")
                    )
            })
        });
        if failed {
            status.push(format!("Covering test failed: {}", test.name));
        } else if stop
            .risk
            .as_ref()
            .is_some_and(|risk| risk.kind == RiskKind::TestChanged)
        {
            status.push(format!(
                "Covering test changed: {} {}",
                test.path, test.name
            ));
        }
    }
    if app.guide_current_changed() {
        status.push("Changed since you checked".to_string());
    }
    if !app.guide_cursor_in_stop() {
        status.push("Off stop".to_string());
    }
    status
}

pub(super) fn render_guide_details(frame: &mut Frame, app: &mut App) {
    let Some(stop) = app.current_stop() else {
        crate::ui::help_popup::render_text_details(
            frame,
            app,
            "Guide stop",
            "This stop is no longer in the guide.",
            false,
        );
        return;
    };
    let mut sections = vec![stop.claim.clone(), stop_location(app, stop)];
    sections.extend(stop_notices(app, stop));
    if !stop.explanation.is_empty() {
        sections.push(stop.explanation.clone());
    }
    if let Some(summary) = app.guide_story_summary() {
        sections.push(format!("Story context\n{summary}"));
    }
    if let Some(guide) = app.active_guide().filter(|guide| !guide.summary.is_empty()) {
        sections.push(format!("Review context\n{}", guide.summary));
    }
    crate::ui::help_popup::render_text_details(
        frame,
        app,
        "Guide stop",
        &sections.join("\n\n"),
        false,
    );
}

fn render_entry_cue(frame: &mut Frame, app: &App, mut area: Rect) -> Rect {
    let Some((guide, cue)) = app.guide_entry_cue().filter(|_| area.height > 1) else {
        return area;
    };
    let (label, action) = match cue {
        GuideEntryCue::Start => ("Review ready", ". start"),
        GuideEntryCue::Resume => ("Review paused", ". resume"),
    };
    let count = guide.stop_count();
    let estimate = guide.estimate_minutes.map_or_else(
        || ", no estimate".to_string(),
        |minutes| format!(", about {minutes} min"),
    );
    let note = if app.installed_guide_is_stale() {
        " · Earlier diff"
    } else if app.installed_guide_has_older_rules() {
        " · Older rules"
    } else if !guide.provenance.audited {
        " · Not audited"
    } else {
        ""
    };
    let width = usize::from(area.width);
    let full = format!("{label}: {count} stops{estimate} · {action}{note}");
    let text = if full.width() <= width {
        full
    } else {
        // When there is not room for the long label, put the action first:
        // narrow panes never hide the only way in.
        let estimate = guide
            .estimate_minutes
            .map_or_else(String::new, |n| format!(" · ~{n} min"));
        let compact = format!("{action}{note} · {count} stops{estimate}");
        if compact.width() <= width {
            compact
        } else {
            format!("{action}{note}")
        }
    };
    draw_line(
        frame,
        take_top(&mut area, 1),
        &text,
        styles::dim_style(&app.theme),
    );
    area
}

fn stop_keys(explicit: bool, width: u16, height: u16) -> (&'static str, Option<&'static str>) {
    let full = if explicit {
        "K checks out  x wrong  p park  , prev  ' return  t map  a ask"
    } else {
        ". next  K checks out  x wrong  p park  , prev  ' return  t map  a ask"
    };
    if usize::from(width) >= full.len() {
        (full, None)
    } else if width >= 33 && height >= 3 {
        let first = if explicit {
            "K checks out  x wrong  p park"
        } else {
            ". next  K pass  x wrong  p park"
        };
        (first, Some(", prev  ' return  t map  a ask"))
    } else if explicit {
        ("K x p  , ' t a", None)
    } else {
        (". K x p  , ' t a", None)
    }
}

fn draw_stop_keys(
    frame: &mut Frame,
    area: Rect,
    text: &'static str,
    tone: &styles::ReviewEmphasis,
) {
    if text.split_ascii_whitespace().all(|word| word.len() == 1) {
        draw_line(frame, area, text, tone.secondary);
        return;
    }
    let mut spans = Vec::new();
    for group in text.split("  ") {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        let (key, label) = group.split_at(1);
        spans.push(Span::styled(key, tone.secondary));
        spans.push(Span::styled(label, tone.muted));
    }
    frame.render_widget(Line::from(spans), area);
}

fn horizontal_rule(app: &App, width: u16) -> Line<'static> {
    Line::styled(
        "─".repeat(usize::from(width)),
        styles::border_style(&app.theme, false),
    )
}

pub(super) fn paint_guide_spotlight(frame: &mut Frame, app: &App) {
    if app.spot_position().is_some() || !app.guide_cursor_in_stop() {
        return;
    }
    let Some(inner) = app.diff_inner_area else {
        return;
    };
    let editor = app.comment_input_annotation_offset;
    let rendered_rows = app
        .diff_row_to_annotation
        .iter()
        .copied()
        .take(usize::from(inner.height));
    if !rendered_rows
        .clone()
        .any(|row| spotlight_annotation(row, editor) == Some(app.diff_state.cursor_line))
    {
        return;
    }
    let theme = &app.theme;
    let panel = theme.palette_panel_bg();
    let mut palette = HashMap::new();
    let mut last_colors = None;
    let mut geometry = None;
    let marker_style = styles::dim_style(theme);
    let unified = app.diff_view_mode == DiffViewMode::Unified;
    let flow =
        unified && app.diff_state.wrap_lines && app.wrap_style == crate::app::WrapStyle::Flow;
    let scroll = if unified {
        app.diff_state.scroll_x.min(usize::from(inner.width)) as u16
    } else {
        0
    };
    let gutter_offset = if unified { 1 } else { 2 };
    let selection = app.visual_selection.map(|selection| selection.ordered());
    let mut previous_row = None;
    let mut block_side = None;
    let mut keep_full_row = false;
    let mut has_search = false;
    let mut has_selection = false;
    let buffer = frame.buffer_mut();
    for (visible, index) in rendered_rows.enumerate() {
        let Some(row) = spotlight_annotation(index, editor) else {
            continue;
        };
        let continuation = previous_row.replace(row) == Some(row);
        if !continuation {
            block_side = app.guide_highlight_row(row);
            keep_full_row = block_side.is_none()
                && (matches!(
                    app.line_annotations.get(row),
                    Some(crate::app::AnnotatedLine::FileHeader { .. })
                ) || app.guide_comment_in_stop(row));
            has_search = app.search_paint_at(row).is_some();
            has_selection = selection.is_some_and(|(first, last)| {
                (first.annotation_idx..=last.annotation_idx).contains(&row)
            });
        }
        let y = inner.y + visible as u16;
        if let Some(side) = block_side {
            let geom = geometry.get_or_insert_with(|| app.pane_geometry(inner, side));
            // Flow wrap can start continuation text in the gutter columns.
            // Only paint a blank gutter cell and leave its normal background.
            if !(flow && continuation)
                && geom.content_width != 0
                && let Some(column) = geom.content_x_start.checked_sub(scroll + gutter_offset)
                && column >= inner.x
                && let Some(cell) = buffer.cell_mut((column, y))
                && cell.symbol() == " "
            {
                cell.set_symbol("│").set_style(marker_style);
            }
        } else if !keep_full_row {
            for x in inner.left()..inner.right() {
                let cell = &mut buffer[(x, y)];
                if has_search && cell.bg == theme.search_match_bg
                    || has_selection && cell.bg == theme.bg_highlight
                {
                    continue;
                }
                let colors = (cell.fg, cell.bg);
                let muted = match last_colors {
                    Some((previous, muted)) if previous == colors => muted,
                    _ => {
                        let muted = *palette.entry(colors).or_insert_with(|| {
                            styles::guide_surround_colors(colors.0, colors.1, panel)
                        });
                        last_colors = Some((colors, muted));
                        muted
                    }
                };
                (cell.fg, cell.bg) = muted;
            }
        }
    }
}

/// The live inline editor inserts rendered rows without corresponding diff
/// annotations. Leave the editor itself at normal contrast.
fn spotlight_annotation(row: usize, editor: Option<(usize, usize, usize)>) -> Option<usize> {
    match editor {
        Some((start, len, replaced)) if row >= start => {
            (row >= start + len).then(|| row - len + replaced)
        }
        _ => Some(row),
    }
}

fn take_top(area: &mut Rect, height: u16) -> Rect {
    let height = height.min(area.height);
    let top = Rect::new(area.x, area.y, area.width, height);
    area.y += height;
    area.height -= height;
    top
}

fn short_line(text: &str, width: u16, style: Style) -> Line<'static> {
    Line::styled(
        truncate_str(text.lines().next().unwrap_or_default(), usize::from(width)),
        style,
    )
}

fn draw_line(frame: &mut Frame, area: Rect, text: &str, style: Style) {
    if !area.is_empty() {
        frame.render_widget(short_line(text, area.width, style), area);
    }
}

fn draw_wrapped(frame: &mut Frame, area: &mut Rect, text: &str, style: Style, limit: u16) {
    if area.is_empty() || text.is_empty() {
        return;
    }
    let paragraph = Paragraph::new(text).style(style).wrap(Wrap { trim: true });
    let height = paragraph.line_count(area.width).min(usize::from(limit)) as u16;
    frame.render_widget(paragraph, take_top(area, height));
}

fn prose(text: &str, width: u16) -> Vec<Line<'_>> {
    if width == 0 {
        return Vec::new();
    }
    text.lines()
        .flat_map(|line| wrap_spans(&[Span::raw(line.trim())], usize::from(width)))
        .map(|mut spans| {
            if let Some(span) = spans.first_mut()
                && let std::borrow::Cow::Owned(text) = &mut span.content
            {
                text.truncate(text.trim_end().len());
                let leading = text.len() - text.trim_start().len();
                text.drain(..leading);
            }
            Line::from(spans)
        })
        .collect()
}

fn clipped_prose<'a>(
    text: &'a str,
    width: u16,
    limit: u16,
    suffix: &'static str,
) -> impl Iterator<Item = Line<'a>> {
    clip_prose(prose(text, width), width, limit, suffix).into_iter()
}

fn clip_prose<'a>(
    mut rows: Vec<Line<'a>>,
    width: u16,
    limit: u16,
    suffix: &'static str,
) -> Vec<Line<'a>> {
    if rows.len() <= usize::from(limit) {
        return rows;
    }
    rows.truncate(usize::from(limit));
    if let Some(line) = rows.last_mut() {
        // Prose has a single unstyled span. Trim on grapheme boundaries before
        // appending the cue, rather than painting over the last terminal cells.
        let suffix = if suffix.width() <= usize::from(width) {
            suffix
        } else {
            "…"
        };
        let budget = usize::from(width).saturating_sub(suffix.width() + 1);
        let prefix = line.spans.first().map_or("", |span| {
            let text = span.content.as_ref();
            let kept = prefix_within_width(text, budget);
            if !kept.ends_with(char::is_whitespace)
                && text[kept.len()..].starts_with(|ch: char| !ch.is_whitespace())
                && let Some((complete, _)) = kept.rsplit_once(char::is_whitespace)
            {
                complete.trim_end()
            } else {
                kept.trim_end()
            }
        });
        *line = if prefix.is_empty() {
            Line::from(suffix)
        } else {
            Line::from(format!("{prefix} {suffix}"))
        };
    }
    rows
}

fn draw_prose(
    frame: &mut Frame,
    area: &mut Rect,
    rows: Vec<Line<'_>>,
    style: Style,
    limit: u16,
    suffix: &'static str,
) {
    let lines = clip_prose(rows, area.width, limit.min(area.height), suffix);
    if !lines.is_empty() {
        let height = lines.len() as u16;
        frame.render_widget(Paragraph::new(lines).style(style), take_top(area, height));
    }
}
