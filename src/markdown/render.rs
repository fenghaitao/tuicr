//! `pulldown-cmark` in, themed pre-wrapped lines out, each mapped to its source lines.
//!
//! Adapted from herdr-reviewr's `src/markdown.rs` at revision
//! `4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8` (Copyright (c) 2026 Dmitry
//! Persiyanov, MIT; see `LICENSE-reviewr` in this directory). Local adaptations:
//!
//! - Styles come from [`MarkdownStyles`] (tuicr's theme and the syntect-derived
//!   Markdown palette) instead of reviewr's `Palette`/`Ink`/`Fill` roles, and
//!   fenced code is highlighted by tuicr's `SyntaxHighlighter`.
//! - Clickable link spans, heading slugs, collapsible `<details>`, severity
//!   badges, and `<relative-time>` are dropped: tuicr renders links as text and
//!   shows every `<details>` body expanded under its summary.
//! - Per-row "own line" tracking is dropped; a row maps to its block's range.
//!   Code-block rows all map to the whole fenced block, which tuicr treats as
//!   one review unit.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::MarkdownStyles;
use crate::syntax::SyntaxHighlighter;

/// The deepest block indent, so nesting never squeezes the content to nothing.
const MAX_NEST: usize = 8;

/// The indent code-block lines carry inside their block.
const CODE_INDENT: &str = "  ";

/// The narrowest a table column shrinks to, unless naturally narrower.
const COL_FLOOR: usize = 8;

/// Rendered markdown: styled lines and their metadata in lockstep.
#[derive(Clone, Debug, Default)]
pub(super) struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub meta: Vec<LineMeta>,
    /// Source lines whose content renders nothing, wholly or in part.
    pub silent: Vec<usize>,
}

/// One rendered line's 1-based source range; ranges of different blocks never overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LineMeta {
    pub source_line: usize,
    pub source_end: usize,
    /// Whether the line is the blank gap set above its block, not its content.
    pub gap: bool,
}

/// Render `text` as styled lines wrapped to `width` columns.
pub(super) fn render(
    text: &str,
    width: usize,
    hl: &SyntaxHighlighter,
    st: &MarkdownStyles,
) -> Rendered {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let mut line_starts = vec![0usize];
    line_starts.extend(
        text.char_indices()
            .filter(|(_, c)| *c == '\n')
            .map(|(i, _)| i + 1),
    );
    let mut r = Renderer {
        hl,
        st,
        width: width.max(1),
        source: text,
        line_starts,
        block_src: 1,
        block_end: 1,
        covered: Vec::new(),
        open_blocks: Vec::new(),
        comment_lines: Vec::new(),
        in_comment: false,
        event_lines: (1, 1),
        inline_span: None,
        marker_line: None,
        out: Rendered::default(),
        inline: Vec::new(),
        styles: Vec::new(),
        quote: 0,
        lists: Vec::new(),
        marker: None,
        links: Vec::new(),
        images: Vec::new(),
        in_heading: false,
        code: None,
        table: None,
        needs_blank: false,
        details: Vec::new(),
    };
    for (event, range) in Parser::new_ext(text, opts).into_offset_iter() {
        r.event(event, range);
    }
    r.flush_block(false);
    r.finish(text)
}

/// One styled run of unwrapped inline text; `"\n"` forces a break.
#[derive(Clone, Debug)]
struct Chunk {
    text: String,
    style: Style,
}

/// An in-progress code block.
struct CodeBlock {
    lang: Option<String>,
    content: String,
}

/// An in-progress table; `range` is the source the wide-table fallback shows.
struct Table {
    range: Range<usize>,
    rows: Vec<Vec<Vec<Chunk>>>,
    row_lines: Vec<(usize, usize)>,
    head_rows: usize,
}

/// An open `<details>`: its summary while the `<summary>` element is being read.
struct DetailsFrame {
    summary: String,
    collecting_summary: bool,
}

struct Renderer<'a> {
    hl: &'a SyntaxHighlighter,
    st: &'a MarkdownStyles,
    width: usize,
    source: &'a str,
    /// Byte offset of each 1-based source line's start, for offset → line mapping.
    line_starts: Vec<usize>,
    /// The 1-based source lines the block being emitted spans.
    block_src: usize,
    block_end: usize,
    /// The ranges of blocks that rendered a line, so their fences and markers count as shown.
    covered: Vec<(usize, usize)>,
    /// The open blocks: closing tag, source lines, and line count at the start.
    open_blocks: Vec<(TagEnd, usize, usize, usize)>,
    /// Lines holding HTML comment text: hidden even inside a block that renders.
    comment_lines: Vec<(usize, usize)>,
    /// An HTML comment opened on an earlier line of the same HTML block, not yet closed.
    in_comment: bool,
    /// The source lines of the event being handled.
    event_lines: (usize, usize),
    /// The source lines that fed the pending inline run.
    inline_span: Option<(usize, usize)>,
    /// The pending item marker's line, shown by the run that takes the marker.
    marker_line: Option<usize>,
    out: Rendered,
    inline: Vec<Chunk>,
    /// The emphasis/link/heading style stack; the current style folds base + every entry.
    styles: Vec<Style>,
    quote: usize,
    /// One entry per open list: the next ordinal for an ordered list, `None` for bullets.
    lists: Vec<Option<u64>>,
    /// The pending item marker, consumed by the item's first flushed line.
    marker: Option<String>,
    /// Open links: destination and the chunk index where the link's text starts.
    links: Vec<(String, usize)>,
    /// Open images: alt-text start chunk and destination.
    images: Vec<(usize, String)>,
    /// Whether a heading's style is on the stack, so a stray `</h1>` pops nothing.
    in_heading: bool,
    code: Option<CodeBlock>,
    table: Option<Table>,
    needs_blank: bool,
    details: Vec<DetailsFrame>,
}

/// Whether a block's whole source range shows once it renders a line.
fn shows_whole(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::Item
            | Tag::CodeBlock(_)
            | Tag::Table(_)
            | Tag::HtmlBlock
    )
}

impl Renderer<'_> {
    fn collecting_summary(&self) -> bool {
        self.details.last().is_some_and(|d| d.collecting_summary)
    }

    fn append_summary(&mut self, text: &str) {
        if let Some(d) = self.details.last_mut() {
            d.summary.push_str(text);
        }
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        self.event_lines = (self.src_line(range.start), self.end_line(&range));
        // A comment never outlives its HTML block: pulldown ends the block by then.
        if matches!(
            event,
            Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock)
        ) {
            self.in_comment = false;
        }
        // A tight item's pending text flushes first, never counting as the next block's line.
        if let Event::Start(tag) = &event
            && shows_whole(tag)
        {
            if matches!(tag, Tag::HtmlBlock) && !self.collecting_summary() {
                self.flush_block(true);
            }
            let (start, end) = self.event_lines;
            self.open_blocks
                .push((tag.to_end(), start, end, self.out.lines.len()));
        }
        let closes = match &event {
            Event::End(end) => self.open_blocks.last().is_some_and(|b| b.0 == *end),
            _ => false,
        };
        self.dispatch(event, range);
        if closes
            && let Some((_, start, end, before)) = self.open_blocks.pop()
            && self.out.lines.len() > before
        {
            self.covered.push((start, end));
        }
    }

    fn dispatch(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::InlineHtml(t) => {
                self.handle_html(&t, true, &range);
            }
            Event::Html(t) => {
                if !self.collecting_summary() {
                    // A tight item's pending text emits first, so HTML never jumps ahead.
                    self.flush_block(true);
                }
                self.block_src = self.src_line(range.start);
                self.block_end = self.end_line(&range);
                self.handle_html(&t, false, &range);
            }
            Event::Start(tag) if !self.collecting_summary() => self.start(tag, range),
            Event::End(tag) if !self.collecting_summary() => self.end(tag),
            Event::Text(t) => {
                if self.collecting_summary() {
                    self.append_summary(&t);
                } else if let Some(code) = &mut self.code {
                    code.content.push_str(&t);
                } else {
                    self.push_text(&t, self.current_style());
                }
            }
            Event::Code(t) if !self.collecting_summary() => {
                // Inline code sits on a chip, so it reads as code in every theme.
                let style = self.current_style().patch(self.st.inline_code);
                self.push_text(&t, style);
            }
            Event::SoftBreak if self.collecting_summary() => self.append_summary(" "),
            Event::SoftBreak => self.push_text(" ", self.current_style()),
            Event::HardBreak if !self.collecting_summary() => {
                let style = self.current_style();
                self.push_chunk("\n".into(), style);
            }
            Event::Rule => {
                self.flush_block(true);
                self.block_src = self.src_line(range.start);
                self.block_end = self.block_src;
                self.blank_before_block();
                let budget = self.budget(self.prefix(None).0.width());
                let line = Line::from(vec![
                    self.prefix(None).0,
                    Span::styled("─".repeat(budget), self.st.border),
                ]);
                self.push_line(line);
                self.needs_blank = true;
            }
            Event::TaskListMarker(done) => {
                self.marker = Some(if done { "☑ ".into() } else { "☐ ".into() });
            }
            // Footnotes and math are silent.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        // A block-level tag stamps the source line its rendered lines map to.
        if matches!(
            tag,
            Tag::Paragraph | Tag::Heading { .. } | Tag::Item | Tag::CodeBlock(_) | Tag::Table(_)
        ) {
            self.block_src = self.src_line(range.start);
            self.block_end = self.end_line(&range);
        }
        match tag {
            Tag::Heading { .. } => {
                self.in_heading = true;
                self.styles.push(self.st.heading);
            }
            Tag::BlockQuote(_) => {
                self.flush_block(true);
                self.quote += 1;
            }
            Tag::List(start) => {
                // "- a" followed by a nested list flushes "a" before the depth changes.
                self.flush_block(false);
                self.lists.push(start);
            }
            Tag::Item => {
                self.marker_line = Some(self.event_lines.0);
                self.marker = Some(match self.lists.last().copied().flatten() {
                    Some(n) => {
                        if let Some(slot) = self.lists.last_mut() {
                            *slot = Some(n + 1);
                        }
                        format!("{n}. ")
                    }
                    None => "• ".into(),
                });
            }
            Tag::CodeBlock(kind) => {
                self.flush_block(true);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code = Some(CodeBlock {
                    lang,
                    content: String::new(),
                });
            }
            Tag::Emphasis => {
                let italic = self.st.italic;
                self.push_style(|s| s.patch(italic).add_modifier(Modifier::ITALIC));
            }
            Tag::Strong => {
                let bold = self.st.bold;
                self.push_style(|s| s.patch(bold).add_modifier(Modifier::BOLD));
            }
            Tag::Strikethrough => {
                let strike = self.st.strike;
                self.push_style(|s| s.patch(strike).add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                let at = self.chunk_len();
                self.links.push((dest_url.to_string(), at));
                let link = self.st.link;
                self.push_style(|s| s.patch(link).add_modifier(Modifier::UNDERLINED));
            }
            Tag::Image { dest_url, .. } => {
                let at = self.chunk_len();
                self.images.push((at, dest_url.to_string()));
            }
            Tag::Table(_) => {
                self.flush_block(true);
                self.table = Some(Table {
                    range,
                    rows: Vec::new(),
                    row_lines: Vec::new(),
                    head_rows: 0,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                let lines = (self.src_line(range.start), self.end_line(&range));
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                    t.row_lines.push(lines);
                }
            }
            Tag::TableCell => {
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(Vec::new());
                }
            }
            // Paragraph / FootnoteDefinition / HtmlBlock carry no styling of their own.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush_block(true),
            TagEnd::Heading(_) => self.end_heading(),
            TagEnd::BlockQuote(_) => {
                self.flush_block(true);
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.flush_block(false);
                self.lists.pop();
                self.needs_blank = true;
            }
            TagEnd::Item => {
                self.flush_block(false);
                self.marker = None;
            }
            TagEnd::CodeBlock => self.end_code(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => self.end_link(),
            TagEnd::Image => self.end_image(),
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.head_rows = t.rows.len();
                }
            }
            TagEnd::Table => self.end_table(),
            _ => {}
        }
    }

    // ---- inline accumulation -----------------------------------------------------------

    /// Where new chunks land: the open table cell, or the block's inline run.
    fn chunks_mut(&mut self) -> &mut Vec<Chunk> {
        match self
            .table
            .as_mut()
            .and_then(|t| t.rows.last_mut())
            .and_then(|r| r.last_mut())
        {
            Some(cell) => cell,
            None => &mut self.inline,
        }
    }

    fn chunk_len(&mut self) -> usize {
        self.chunks_mut().len()
    }

    fn push_text(&mut self, text: &str, style: Style) {
        let clean = sanitize(text);
        self.push_chunk(clean, style);
    }

    fn push_chunk(&mut self, text: String, style: Style) {
        if self.table.is_none() {
            let (start, end) = self.event_lines;
            self.inline_span = Some(match self.inline_span {
                Some((s, e)) => (s.min(start), e.max(end)),
                None => (start, end),
            });
        }
        self.chunks_mut().push(Chunk { text, style });
    }

    fn push_style(&mut self, patch: impl FnOnce(Style) -> Style) {
        self.styles.push(patch(self.current_style()));
    }

    fn current_style(&self) -> Style {
        self.styles.last().copied().unwrap_or(if self.quote > 0 {
            self.st.text.patch(self.st.quote)
        } else {
            self.st.text
        })
    }

    /// Close a link, appending its destination dim when the text differs.
    fn end_link(&mut self) {
        self.styles.pop();
        let Some((dest, start)) = self.links.pop() else {
            return;
        };
        let text: String = self.chunks_mut()[start..]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        if !dest.is_empty() && text != dest {
            let style = self.st.muted;
            self.push_chunk(format!(" ({})", sanitize(&dest)), style);
        }
    }

    /// Collapse an image to a dim `⧉ alt-text` placeholder; nothing is fetched.
    fn end_image(&mut self) {
        let Some((start, _dest)) = self.images.pop() else {
            return;
        };
        let chunks = self.chunks_mut();
        let alt: String = chunks[start..].iter().map(|c| c.text.as_str()).collect();
        chunks.truncate(start);
        self.emit_image(alt.trim());
    }

    fn emit_image(&mut self, alt: &str) {
        let alt = sanitize(alt);
        let text = if alt.is_empty() {
            "⧉ image".to_string()
        } else {
            format!("⧉ {alt}")
        };
        let style = self.st.muted;
        self.push_chunk(text, style);
    }

    /// Handle one HTML event. Returns whether it holds comment text, which never renders.
    fn handle_html(&mut self, html: &str, inline: bool, range: &Range<usize>) -> bool {
        let lines = (self.src_line(range.start), self.end_line(range));
        let mut html = html;
        let mut commented = false;
        // A comment an earlier HTML line opened swallows everything up to its close.
        if self.in_comment {
            commented = true;
            match html.find("-->") {
                Some(at) => {
                    html = &html[at + 3..];
                    self.in_comment = false;
                }
                None => html = "",
            }
        }
        if let Some(open) = html.rfind("<!--")
            && !html[open..].contains("-->")
        {
            self.in_comment = true;
        }
        let produced = self.inline.len();
        for tok in html_tokens(html) {
            match tok {
                HtmlTok::Comment => commented = true,
                HtmlTok::Text(t) => {
                    if self.collecting_summary() {
                        self.append_summary(t);
                    } else if !t.trim().is_empty() {
                        self.push_text(t, self.current_style());
                    }
                }
                HtmlTok::Tag {
                    name,
                    closing,
                    self_closing,
                    raw,
                } => {
                    self.handle_html_tag(name, closing, self_closing, raw);
                }
            }
        }
        if commented {
            self.comment_lines.push(lines);
        }
        if !inline && self.inline.len() > produced && !self.collecting_summary() {
            self.flush_block(true);
        }
        commented
    }

    fn handle_html_tag(&mut self, name: &str, closing: bool, self_closing: bool, raw: &str) {
        let name = name.trim().to_ascii_lowercase();
        match (name.as_str(), closing, self_closing) {
            ("details", false, false) => {
                self.flush_block(true);
                self.details.push(DetailsFrame {
                    summary: String::new(),
                    collecting_summary: false,
                });
            }
            ("details", true, _) => {
                self.flush_block(true);
                self.details.pop();
            }
            ("summary", false, _) => {
                if let Some(d) = self.details.last_mut() {
                    d.collecting_summary = true;
                    d.summary.clear();
                }
            }
            ("summary", true, _) => self.finish_summary(),
            ("br", _, _) if !self.collecting_summary() => {
                let style = self.current_style();
                self.push_chunk("\n".into(), style);
            }
            ("img", _, _) if !self.collecting_summary() => {
                let alt = html_attr(raw, "alt").unwrap_or_default();
                self.emit_image(&alt);
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false, false)
                if !self.collecting_summary() =>
            {
                self.flush_block(true);
                self.in_heading = true;
                self.styles.push(self.st.heading);
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true, _) => self.end_heading(),
            _ => {}
        }
    }

    fn end_heading(&mut self) {
        if !std::mem::take(&mut self.in_heading) {
            return;
        }
        self.flush_block(true);
        self.styles.pop();
    }

    /// A `<details>` summary renders as a `▾ summary` line; the body follows expanded.
    fn finish_summary(&mut self) {
        let Some(d) = self.details.last_mut() else {
            return;
        };
        d.collecting_summary = false;
        let summary = d.summary.split_whitespace().collect::<Vec<_>>().join(" ");
        let summary = sanitize(&summary);
        self.flush_block(true);
        self.block_src = self.event_lines.0;
        self.block_end = self.event_lines.1;
        self.blank_before_block();
        let (first, _) = self.prefix(None);
        self.push_line(Line::from(vec![
            first,
            Span::styled("▾ ", self.st.muted),
            Span::styled(summary, self.st.text),
        ]));
        self.needs_blank = true;
    }

    // ---- block emission ------------------------------------------------------------------

    /// The `(first, continuation)` block prefixes, the marker on the first.
    fn prefix(&self, marker: Option<&str>) -> (Span<'static>, Span<'static>) {
        let bars = "▎".repeat(self.quote.min(MAX_NEST));
        let gap = if bars.is_empty() { "" } else { " " };
        let indent = "  ".repeat(self.lists.len().min(MAX_NEST).saturating_sub(1));
        let marker = marker.unwrap_or("");
        let first = format!("{bars}{gap}{indent}{marker}");
        let cont = format!("{bars}{gap}{indent}{}", " ".repeat(marker.width()));
        (
            Span::styled(first, self.st.muted),
            Span::styled(cont, self.st.muted),
        )
    }

    /// Content columns left of a `prefix_width`-wide prefix.
    fn budget(&self, prefix_width: usize) -> usize {
        self.width.saturating_sub(prefix_width).max(1)
    }

    /// The 1-based source line holding byte offset `at`.
    fn src_line(&self, at: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= at)
    }

    /// The last source line of an event's end-exclusive byte range.
    fn end_line(&self, range: &Range<usize>) -> usize {
        self.src_line(range.end.saturating_sub(1).max(range.start))
    }

    /// Settle the source ranges, each ending where the next begins, and the silent lines.
    fn finish(mut self, text: &str) -> Rendered {
        // Blank once container markers go: a `>`-only line hides nothing.
        let blank: Vec<bool> = text
            .lines()
            .map(|l| {
                l.trim_start_matches(|c: char| c == '>' || c.is_whitespace())
                    .is_empty()
            })
            .collect();
        let is_blank = |line: usize| blank.get(line.wrapping_sub(1)).copied().unwrap_or(true);
        let meta = &mut self.out.meta;
        let mut next_start = usize::MAX;
        for i in (0..meta.len()).rev() {
            if let Some(after) = meta.get(i + 1)
                && after.source_line > meta[i].source_line
            {
                next_start = after.source_line;
            }
            let m = &mut meta[i];
            let mut end = m
                .source_end
                .min(next_start.saturating_sub(1))
                .max(m.source_line);
            // A block's range can run through the blank lines after it; it ends at its text.
            while end > m.source_line && is_blank(end) {
                end -= 1;
            }
            m.source_end = end;
        }
        let mut shown = vec![false; blank.len() + 2];
        let spans = meta
            .iter()
            .filter(|m| !m.gap)
            .map(|m| (m.source_line, m.source_end))
            .chain(self.covered);
        for (start, end) in spans {
            for line in start..=end.min(shown.len() - 1) {
                shown[line] = true;
            }
        }
        let mut hidden = vec![false; shown.len()];
        for (start, end) in self.comment_lines {
            for line in start..=end.min(hidden.len() - 1) {
                hidden[line] = true;
            }
        }
        self.out.silent = (1..=blank.len())
            .filter(|&line| hidden[line] || (!is_blank(line) && !shown[line]))
            .collect();
        self.out
    }

    /// Emit one rendered line mapped to the current block.
    fn push_line(&mut self, line: Line<'static>) {
        self.out.meta.push(LineMeta {
            source_line: self.block_src,
            source_end: self.block_end.max(self.block_src),
            gap: false,
        });
        self.out.lines.push(line);
    }

    /// A blank line between blocks.
    fn blank_before_block(&mut self) {
        if self.needs_blank && !self.out.lines.is_empty() {
            let bars = "▎".repeat(self.quote.min(MAX_NEST));
            self.out.meta.push(LineMeta {
                source_line: self.block_src,
                source_end: self.block_src,
                gap: true,
            });
            self.out.lines.push(if bars.is_empty() {
                Line::default()
            } else {
                Line::from(Span::styled(bars, self.st.muted))
            });
        }
        self.needs_blank = false;
    }

    /// Wrap and emit the pending inline run; `set_blank` puts a gap before the next block.
    fn flush_block(&mut self, set_blank: bool) {
        let span = self.inline_span.take();
        if self.inline.iter().all(|c| c.text.trim().is_empty()) {
            self.inline.clear();
            if set_blank {
                self.needs_blank = true;
            }
            return;
        }
        // The run maps to the lines that fed it, marker line included.
        let stamp = (self.block_src, self.block_end);
        if let Some((mut start, mut end)) = span {
            if self.marker.is_some()
                && let Some(line) = self.marker_line.take()
            {
                start = start.min(line);
                end = end.max(line);
            }
            self.block_src = start;
            self.block_end = end;
        }
        self.blank_before_block();
        let marker = self.marker.take();
        let (first, cont) = self.prefix(marker.as_deref());
        let fragments: Vec<Fragment> = std::mem::take(&mut self.inline)
            .into_iter()
            .map(|c| (c.text, c.style))
            .collect();
        let wrapped = wrap_fragments(&fragments, self.budget(first.width()), true);
        for (i, spans) in wrapped.into_iter().enumerate() {
            let prefix = if i == 0 { first.clone() } else { cont.clone() };
            let mut line = vec![prefix];
            line.extend(spans);
            self.push_line(Line::from(line));
        }
        (self.block_src, self.block_end) = stamp;
        if set_blank {
            self.needs_blank = true;
        }
    }

    /// Take the pending item marker for a non-inline block, its line with it.
    fn take_marker(&mut self) -> Option<String> {
        let marker = self.marker.take();
        if marker.is_some()
            && let Some(line) = self.marker_line.take()
        {
            self.covered.push((line, line));
        }
        marker
    }

    /// Emit styled fragments char-wrapped under the block prefix, a pending marker first.
    fn emit_fragments(&mut self, fragments: Vec<Fragment>, extra_indent: &str) {
        let marker = self.take_marker();
        let (first, cont) = self.prefix(marker.as_deref());
        let first = Span::styled(format!("{}{extra_indent}", first.content), self.st.muted);
        let cont = Span::styled(format!("{}{extra_indent}", cont.content), self.st.muted);
        let budget = self.budget(cont.width());
        let wrapped = wrap_fragments(&fragments, budget, false);
        if wrapped.is_empty() {
            self.push_line(Line::from(first));
            return;
        }
        for (i, spans) in wrapped.into_iter().enumerate() {
            let mut line = vec![if i == 0 { first.clone() } else { cont.clone() }];
            line.extend(spans);
            self.push_line(Line::from(line));
        }
    }

    /// Close a code block: highlight it whole, emit it indented and char-wrapped.
    /// Every row maps to the whole block, fences included.
    fn end_code(&mut self) {
        let Some(CodeBlock { lang, content }) = self.code.take() else {
            return;
        };
        self.blank_before_block();
        if lang
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case("mermaid"))
        {
            let text = match mermaid_kind(&content) {
                Some(k) => format!("⧉ mermaid {k}"),
                None => "⧉ mermaid".to_string(),
            };
            self.emit_fragments(vec![(text, self.st.muted)], "");
            self.needs_blank = true;
            return;
        }
        let content = content.replace('\t', "    ");
        let highlighted = lang
            .as_deref()
            .and_then(|token| self.hl.highlight_fenced_code(token, &content))
            .unwrap_or_else(|| {
                content
                    .lines()
                    .map(|line| vec![(self.st.code_text, line.to_string())])
                    .collect()
            });
        for line in highlighted {
            let fragments: Vec<Fragment> = line
                .into_iter()
                .map(|(style, text)| (sanitize(&text), style))
                .collect();
            self.emit_fragments(fragments, CODE_INDENT);
        }
        self.needs_blank = true;
    }

    /// Close a table: aligned, widest columns shrunk to fit, dim source when floors overflow.
    fn end_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        self.blank_before_block();
        // `first` and `cont` share a width, so the marker never shifts columns.
        let marker = self.take_marker();
        let (first, cont) = self.prefix(marker.as_deref());
        let budget = self.budget(cont.width());

        let cell_text =
            |cell: &[Chunk]| -> String { cell.iter().map(|c| c.text.as_str()).collect() };
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0usize; cols];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell_text(cell).width());
            }
        }
        // Each pass levels the widest columns to the next width or their floor: passes ≤ columns.
        let floors: Vec<usize> = widths.iter().map(|w| (*w).min(COL_FLOOR)).collect();
        let sep_total = 3 * cols.saturating_sub(1);
        let mut deficit = (widths.iter().sum::<usize>() + sep_total).saturating_sub(budget);
        while deficit > 0 {
            let Some(hi) = (0..cols)
                .filter(|&i| widths[i] > floors[i])
                .map(|i| widths[i])
                .max()
            else {
                break;
            };
            let ties: Vec<usize> = (0..cols)
                .filter(|&i| widths[i] == hi && hi > floors[i])
                .collect();
            let step = (0..cols)
                .map(|j| widths[j])
                .filter(|&w| w < hi)
                .chain(ties.iter().map(|&i| floors[i]))
                .max()
                .expect("ties are non-empty");
            let room = (hi - step) * ties.len();
            if room >= deficit {
                let per = deficit / ties.len();
                let extra = deficit % ties.len();
                // The rightmost ties give up the remainder cell, so the split is exact.
                for (k, &i) in ties.iter().rev().enumerate() {
                    widths[i] = hi - per - usize::from(k < extra);
                }
                deficit = 0;
            } else {
                for &i in &ties {
                    widths[i] = step;
                }
                deficit -= room;
            }
        }
        let total: usize = widths.iter().sum::<usize>() + sep_total;

        if deficit > 0 {
            // Over-wide at every floor: the table renders as its source text instead.
            let src = self.source.get(table.range.clone()).unwrap_or("");
            let first_line = self.src_line(table.range.start);
            for (i, src_line) in src.trim_end_matches('\n').split('\n').enumerate() {
                self.block_src = first_line + i;
                self.block_end = self.block_src;
                self.emit_fragments(vec![(sanitize(src_line), self.st.secondary)], "");
            }
            self.needs_blank = true;
            return;
        }

        let dim = self.st.border;
        for (r, row) in table.rows.iter().enumerate() {
            let head = r < table.head_rows;
            let style_of = |c: &Chunk| {
                if head {
                    c.style.add_modifier(Modifier::BOLD)
                } else {
                    c.style
                }
            };
            // Only a cell wider than its column wraps; the row is as tall as its tallest.
            let cells: Vec<Vec<Vec<Span<'static>>>> = (0..cols)
                .map(|i| {
                    let cell = row.get(i).map_or(&[][..], Vec::as_slice);
                    if cell_text(cell).width() <= widths[i] {
                        let spans: Vec<Span<'static>> = cell
                            .iter()
                            .map(|c| Span::styled(c.text.clone(), style_of(c)))
                            .collect();
                        if spans.is_empty() {
                            Vec::new()
                        } else {
                            vec![spans]
                        }
                    } else {
                        let fragments: Vec<Fragment> =
                            cell.iter().map(|c| (c.text.clone(), style_of(c))).collect();
                        wrap_fragments(&fragments, widths[i], true)
                    }
                })
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(0).max(1);
            // Each row maps to its own source lines, so a comment on one row anchors it.
            if let Some(&(start, end)) = table.row_lines.get(r) {
                self.block_src = start;
                self.block_end = end;
            }
            let mut cells: Vec<std::vec::IntoIter<Vec<Span<'static>>>> =
                cells.into_iter().map(Vec::into_iter).collect();
            for l in 0..height {
                let lead = if r == 0 && l == 0 {
                    first.clone()
                } else {
                    cont.clone()
                };
                let mut spans: Vec<Span<'static>> = vec![lead];
                for (i, width) in widths.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::styled(" │ ", dim));
                    }
                    let mut used = 0;
                    if let Some(cell_spans) = cells[i].next() {
                        used = cell_spans.iter().map(Span::width).sum();
                        spans.extend(cell_spans);
                    }
                    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
                }
                self.push_line(Line::from(spans));
            }
            if head && r + 1 == table.head_rows {
                // The header rule stands for the delimiter row, the line under the header.
                self.block_src = self.block_end + 1;
                self.block_end = self.block_src;
                self.push_line(Line::from(vec![
                    cont.clone(),
                    Span::styled("─".repeat(total), dim),
                ]));
            }
        }
        self.needs_blank = true;
    }
}

/// A control or bidi-override character, which the display refuses.
fn hostile_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
}

fn mermaid_kind(content: &str) -> Option<&str> {
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("%%") {
            continue;
        }
        let word = t
            .split(|c: char| c.is_whitespace() || c == '{' || c == '-')
            .next()?;
        if word.is_empty() {
            continue;
        }
        return Some(word);
    }
    None
}

enum HtmlTok<'a> {
    Tag {
        name: &'a str,
        closing: bool,
        self_closing: bool,
        raw: &'a str,
    },
    Text(&'a str),
    Comment,
}

fn html_tokens(s: &str) -> Vec<HtmlTok<'_>> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with("<!--") {
            let end = s[i + 4..].find("-->").map_or(s.len(), |n| i + 4 + n + 3);
            out.push(HtmlTok::Comment);
            i = end;
            continue;
        }
        if bytes[i] == b'<' {
            let end = s[i + 1..].find('>').map_or(s.len(), |n| i + 1 + n + 1);
            let raw = &s[i..end];
            let inner = raw.trim_start_matches('<').trim_end_matches('>');
            let closing = inner.starts_with('/');
            let self_closing = !closing && inner.trim_end().ends_with('/');
            let name_src = inner.trim_start_matches('/').trim_end_matches('/').trim();
            let name_end = name_src
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(name_src.len());
            let name = &name_src[..name_end];
            out.push(HtmlTok::Tag {
                name,
                closing,
                self_closing,
                raw,
            });
            i = end;
            continue;
        }
        let next = s[i..].find('<').map_or(s.len(), |n| i + n);
        if next > i {
            out.push(HtmlTok::Text(&s[i..next]));
        }
        i = next;
    }
    out
}

fn html_attr(tag: &str, key: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{key}=");
    let at = lower.find(&needle)?;
    let rest = tag.get(at + needle.len()..)?;
    let rest = rest.trim_start();
    let bytes = rest.as_bytes();
    if bytes.first().copied() == Some(b'"') {
        let end = rest[1..].find('"')?;
        return Some(rest[1..=end].to_string());
    }
    if bytes.first().copied() == Some(b'\'') {
        let end = rest[1..].find('\'')?;
        return Some(rest[1..=end].to_string());
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Hostile characters as a visible placeholder, tabs as spaces.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => out.push_str("    "),
            '\n' => out.push('\n'),
            c if hostile_char(c) => out.push('�'),
            c => out.push(c),
        }
    }
    out
}

/// One styled run entering the wrapper.
type Fragment = (String, Style);

/// Wrap fragments to `budget` cells, at words when `by_words`, else anywhere.
fn wrap_fragments(
    fragments: &[Fragment],
    budget: usize,
    by_words: bool,
) -> Vec<Vec<Span<'static>>> {
    let mut w = Wrapper {
        budget: budget.max(1),
        lines: Vec::new(),
        line: Vec::new(),
        line_w: 0,
        word: Vec::new(),
        word_w: 0,
    };
    for (text, style) in fragments {
        for ch in text.chars() {
            match ch {
                '\n' => {
                    w.place_word();
                    w.flush_line();
                }
                ' ' if by_words => w.place_word(),
                c if by_words => w.push_word_char(c, *style),
                c => w.place_char(c, *style),
            }
        }
    }
    w.place_word();
    if !w.line.is_empty() {
        w.lines.push(w.line);
    }
    w.lines
}

/// The wrap state; a word can span styled fragments.
struct Wrapper {
    budget: usize,
    lines: Vec<Vec<Span<'static>>>,
    line: Vec<Span<'static>>,
    line_w: usize,
    word: Vec<(String, Style)>,
    word_w: usize,
}

impl Wrapper {
    fn flush_line(&mut self) {
        self.lines.push(std::mem::take(&mut self.line));
        self.line_w = 0;
    }

    /// Append `text`, merging into the last span where the style continues.
    fn place(&mut self, text: &str, style: Style, w: usize) {
        if let Some(last) = self.line.last_mut()
            && last.style == style
        {
            last.content.to_mut().push_str(text);
        } else {
            self.line.push(Span::styled(text.to_string(), style));
        }
        self.line_w += w;
    }

    /// Append one character, breaking the line first when it would overflow.
    fn place_char(&mut self, c: char, style: Style) {
        let w = char_width(c);
        if self.line_w + w > self.budget && self.line_w > 0 {
            self.flush_line();
        }
        self.place(c.encode_utf8(&mut [0u8; 4]), style, w);
    }

    /// Grow the pending word by one character.
    fn push_word_char(&mut self, c: char, style: Style) {
        if let Some((t, s)) = self.word.last_mut()
            && *s == style
        {
            t.push(c);
        } else {
            self.word.push((c.to_string(), style));
        }
        self.word_w += char_width(c);
    }

    /// Place the word, breaking the line before it, or it, when it doesn't fit.
    fn place_word(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let needs_sep = self.line_w > 0;
        if needs_sep && self.line_w + 1 + self.word_w > self.budget && self.word_w <= self.budget {
            self.flush_line();
        } else if needs_sep {
            // A separator keeps a style only both sides share, so underlines never bleed.
            let next = self.word[0].1;
            let style = match self.line.last() {
                Some(prev) if prev.style == next => next,
                _ => Style::default(),
            };
            self.place(" ", style, 1);
        }
        for (text, style) in std::mem::take(&mut self.word) {
            for ch in text.chars() {
                self.place_char(ch, style);
            }
        }
        self.word_w = 0;
    }
}

fn char_width(c: char) -> usize {
    UnicodeWidthStr::width(c.encode_utf8(&mut [0u8; 4]) as &str)
}
