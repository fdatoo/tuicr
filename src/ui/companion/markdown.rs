use std::borrow::Cow;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::syntax::HighlightedSpans;
use crate::theme::Theme;
use crate::ui::styles::{contrast_ratio, linear_rgb, luminance, relative_luminance};
use crate::vcs::TAB_WIDTH;

// Deeper container nesting must not consume the whole width of a narrow pane.
const MAX_INDENT_LEVELS: usize = 4;

pub(super) fn render(content: &str, theme: &Theme) -> Vec<Line<'static>> {
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES;
    let mut writer = MarkdownWriter::new(theme);
    let mut events = Parser::new_ext(content, options);
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let mut code = String::new();
                for event in events.by_ref() {
                    match event {
                        Event::End(TagEnd::CodeBlock) => break,
                        Event::Text(text) => code.push_str(&text),
                        _ => {}
                    }
                }
                writer.code_block(kind, &code);
            }
            Event::Start(tag) => writer.start(tag),
            Event::End(tag) => writer.end(tag),
            Event::Text(text) | Event::InlineHtml(text) => writer.text(&text, writer.style()),
            Event::Html(html) => {
                // HTML is inert text, never a terminal instruction or a remote resource.
                for (idx, line) in html.lines().enumerate() {
                    if idx > 0 {
                        writer.flush_line();
                    }
                    writer.text(line, writer.style());
                }
                if html.ends_with('\n') {
                    writer.flush_line();
                }
            }
            Event::Code(code) => writer.text(
                &code,
                writer.style().fg(readable_color(
                    theme.comment_note,
                    theme.fg_primary,
                    theme.panel_bg,
                )),
            ),
            Event::SoftBreak => writer.text(" ", writer.style()),
            Event::HardBreak => writer.flush_line(),
            Event::Rule => {
                writer.separate();
                writer.text("───", writer.secondary());
                writer.separate();
            }
            Event::TaskListMarker(checked) => {
                writer.text(if checked { "[x] " } else { "[ ] " }, writer.secondary());
            }
            _ => {}
        }
    }
    writer.flush_line();
    if writer.rows.last().is_some_and(|row| row.spans.is_empty()) {
        writer.rows.pop();
    }
    writer.rows
}

struct MarkdownWriter<'a> {
    theme: &'a Theme,
    rows: Vec<Line<'static>>,
    line: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    quotes: usize,
    table_cells: usize,
}

impl<'a> MarkdownWriter<'a> {
    fn new(theme: &'a Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            line: Vec::new(),
            styles: Vec::new(),
            lists: Vec::new(),
            quotes: 0,
            table_cells: 0,
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        let style = match tag {
            Tag::Heading { .. } | Tag::Strong | Tag::TableHead => {
                self.style().add_modifier(Modifier::BOLD)
            }
            Tag::Emphasis => self.style().add_modifier(Modifier::ITALIC),
            Tag::Strikethrough => self.style().add_modifier(Modifier::CROSSED_OUT),
            Tag::Link { .. } | Tag::Image { .. } => self
                .style()
                .fg(readable_color(
                    self.theme.comment_note,
                    self.theme.fg_primary,
                    self.theme.panel_bg,
                ))
                .add_modifier(Modifier::UNDERLINED),
            Tag::List(start) => {
                self.flush_line();
                self.lists.push(start);
                return;
            }
            Tag::Item => {
                self.start_item();
                return;
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.quotes += 1;
                return;
            }
            Tag::TableCell => {
                if self.table_cells > 0 {
                    self.text(" │ ", self.secondary());
                }
                self.table_cells += 1;
                return;
            }
            _ => return,
        };
        self.styles.push(style);
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.separate();
            }
            TagEnd::TableHead => {
                self.styles.pop();
                self.table_cells = 0;
                self.flush_line();
            }
            TagEnd::Strong
            | TagEnd::Emphasis
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                self.styles.pop();
            }
            TagEnd::Paragraph if !self.lists.is_empty() => self.flush_line(),
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::Table => self.separate(),
            TagEnd::Item => self.flush_line(),
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.separate();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.quotes -= 1;
                if self.quotes == 0 && self.lists.is_empty() {
                    self.separate();
                }
            }
            TagEnd::TableRow => {
                self.table_cells = 0;
                self.flush_line();
            }
            _ => {}
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn secondary(&self) -> Style {
        Style::default().fg(self.theme.fg_secondary)
    }

    fn start_item(&mut self) {
        self.flush_line();
        let indent = "  ".repeat(self.lists.len().saturating_sub(1).min(MAX_INDENT_LEVELS));
        let marker = if let Some(Some(number)) = self.lists.last_mut() {
            let marker = format!("{indent}{number}. ");
            *number = number.saturating_add(1);
            marker
        } else {
            format!("{indent}• ")
        };
        self.text(&marker, self.secondary());
    }

    fn code_block(&mut self, kind: CodeBlockKind<'_>, code: &str) {
        self.separate();
        let lines = code.lines().map(safe_display).collect::<Vec<_>>();
        let mut highlights = match kind {
            CodeBlockKind::Fenced(info) if !info.is_empty() => self
                .theme
                .syntax_highlighter()
                .highlight_fenced_code(&info, &lines)
                .map(Vec::into_iter),
            _ => None,
        };
        for line in lines {
            self.quote_prefix();
            self.line.push(Span::raw("  "));
            if let Some(runs) = highlights.as_mut().and_then(Iterator::next).flatten() {
                self.line.extend(syntax_spans(
                    runs,
                    self.theme.fg_primary,
                    self.theme.panel_bg,
                ));
            } else {
                self.line.push(Span::raw(line.into_owned()));
            }
            self.flush_line();
        }
        self.separate();
    }

    fn text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.quote_prefix();
        let text = safe_display(text);
        if let Some(last) = self.line.last_mut()
            && last.style == style
        {
            last.content.to_mut().push_str(&text);
        } else {
            self.line.push(Span::styled(text.into_owned(), style));
        }
    }

    fn quote_prefix(&mut self) {
        if self.line.is_empty() && self.quotes > 0 {
            self.line.push(Span::styled(
                "│ ".repeat(self.quotes.min(MAX_INDENT_LEVELS)),
                self.secondary(),
            ));
        }
    }

    fn flush_line(&mut self) {
        if !self.line.is_empty() {
            self.rows.push(Line::from(std::mem::take(&mut self.line)));
        }
    }

    fn separate(&mut self) {
        self.flush_line();
        if self.rows.last().is_some_and(|row| !row.spans.is_empty()) {
            self.rows.push(Line::default());
        }
    }
}

/// Sanitize the display projection too: Markdown can decode an entity into a
/// control character even when the provider output itself had none.
pub(super) fn safe_display(text: &str) -> Cow<'_, str> {
    if !text.contains(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut safe = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\t' {
            safe.extend(std::iter::repeat_n(' ', TAB_WIDTH));
        } else if ch.is_control() {
            safe.push('\u{fffd}');
        } else {
            safe.push(ch);
        }
    }
    Cow::Owned(safe)
}

pub(super) fn syntax_spans(
    runs: HighlightedSpans,
    foreground: Color,
    background: Color,
) -> impl Iterator<Item = Span<'static>> {
    runs.into_iter().map(move |(mut style, text)| {
        style.fg = style
            .fg
            .map(|ink| readable_color(ink, foreground, background));
        // Syntect supplies a code background; the dock owns this surface.
        style.bg = None;
        Span::styled(text, style)
    })
}

pub(super) fn readable_color(ink: Color, foreground: Color, background: Color) -> Color {
    let Some(background_luma) = relative_luminance(background) else {
        return ink; // The terminal controls its default background.
    };
    let white = contrast_ratio(1.0, background_luma) >= contrast_ratio(0.0, background_luma);
    if let Some(rgb) = linear_rgb(ink) {
        let luma = luminance(rgb);
        if contrast_ratio(luma, background_luma) >= 4.5 {
            return ink;
        }
        // Blend only as far as necessary, retaining the syntax hue. Linear
        // RGB makes luminance interpolate directly; round toward the endpoint
        // so converting back to terminal RGB cannot lose the target contrast.
        let target = if white {
            (background_luma + 0.05) * 4.5 - 0.05
        } else {
            (background_luma + 0.05) / 4.5 - 0.05
        };
        let endpoint = if white { 1.0 } else { 0.0 };
        let fraction = (target - luma) / (endpoint - luma);
        let [r, g, b] = rgb.map(|channel| {
            let linear = channel + (endpoint - channel) * fraction;
            let srgb = if linear <= 0.0031308 {
                12.92 * linear
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            } * 255.0;
            if white {
                srgb.ceil() as u8
            } else {
                srgb.floor() as u8
            }
        });
        Color::Rgb(r, g, b)
    } else if relative_luminance(foreground)
        .is_some_and(|luma| contrast_ratio(luma, background_luma) >= 4.5)
    {
        foreground
    } else if white {
        Color::White
    } else {
        Color::Black
    }
}
