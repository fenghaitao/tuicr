//! Rendered Markdown documents for the `:markdown` review view.
//!
//! [`render_document`] lays a Markdown source out as pre-wrapped terminal rows
//! and groups them into blocks, each mapped to the inclusive, one-based source
//! lines it came from. Review comments made on a rendered row are stored against
//! those source lines, so a comment is the same whether it was made in rendered
//! or source view, at any width. See `docs/tuicr-rendered-markdown-review.md`.
//!
//! Source lines that render nothing (HTML comments, link reference
//! definitions, ...) are never dropped silently: each contiguous run becomes a
//! marker row of its own, anchored to that run.

mod render;

use std::ops::Range;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::model::LineRange;
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

/// Styles the renderer paints with: tuicr theme colors for prose and chrome,
/// the syntect-derived Markdown palette for constructs, so a preview matches
/// the colors source view uses for the same text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MarkdownStyles {
    pub text: Style,
    pub secondary: Style,
    pub muted: Style,
    pub border: Style,
    pub heading: Style,
    pub bold: Style,
    pub italic: Style,
    pub strike: Style,
    pub link: Style,
    pub quote: Style,
    pub inline_code: Style,
    pub code_text: Style,
}

impl MarkdownStyles {
    pub(crate) fn resolve(theme: &Theme, hl: &SyntaxHighlighter) -> Self {
        let palette = hl.markdown_palette();
        // Only the palette's foreground and modifiers carry over; backgrounds
        // stay the panel's so rows never paint a block of syntect background.
        let fg_only = |style: Style| Style { bg: None, ..style };
        Self {
            text: Style::default().fg(theme.fg_primary),
            secondary: Style::default().fg(theme.fg_secondary),
            muted: Style::default().fg(theme.fg_dim),
            border: Style::default().fg(theme.border_unfocused),
            heading: fg_only(palette.heading).add_modifier(Modifier::BOLD),
            bold: fg_only(palette.bold),
            italic: fg_only(palette.italic),
            strike: fg_only(palette.strike),
            link: fg_only(palette.link),
            quote: fg_only(palette.quote),
            inline_code: fg_only(palette.code).bg(theme.bg_highlight),
            code_text: Style::default().fg(theme.fg_primary),
        }
    }
}

/// A Markdown source laid out at one width: rows in display order and the
/// blocks they group into.
#[derive(Clone, Debug, Default)]
pub struct RenderedDocument {
    pub rows: Vec<RenderedRow>,
    pub blocks: Vec<RenderedBlock>,
}

/// One pre-wrapped display row.
#[derive(Clone, Debug)]
pub struct RenderedRow {
    pub line: Line<'static>,
    /// The row's plain text: what search matches, visual copy takes, and
    /// mouse columns map onto.
    pub text: String,
    /// Index into [`RenderedDocument::blocks`]; `None` for a blank gap row.
    pub block: Option<usize>,
}

/// A run of rows that render one source unit: a paragraph, heading, list
/// item, table row, code block, or a hidden-source marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedBlock {
    /// Inclusive, one-based source lines.
    pub range: LineRange,
    /// The block's rows in [`RenderedDocument::rows`].
    pub rows: Range<usize>,
    /// Whether this is a marker standing for source that renders nothing.
    pub hidden: bool,
}

/// Lay `source` out as rows at most `width` columns wide.
pub fn render_document(source: &str, width: usize, theme: &Theme) -> RenderedDocument {
    let hl = theme.syntax_highlighter();
    let styles = MarkdownStyles::resolve(theme, hl);
    let rendered = render::render(source, width, hl, &styles);
    build_document(rendered, width.max(1), &styles)
}

fn build_document(
    rendered: render::Rendered,
    width: usize,
    styles: &MarkdownStyles,
) -> RenderedDocument {
    let hidden = hidden_runs(&rendered.silent);
    let mut next_hidden = 0;
    let mut doc = RenderedDocument::default();

    for (line, meta) in rendered.lines.into_iter().zip(rendered.meta) {
        if meta.gap {
            doc.push_row(line, None);
            continue;
        }
        let range = LineRange::new(meta.source_line as u32, meta.source_end as u32);
        let continues = doc
            .blocks
            .last()
            .is_some_and(|b| !b.hidden && b.range == range && b.rows.end == doc.rows.len());
        if !continues {
            // Hidden source shows up in source order, ahead of the first block
            // that starts after it. Between blocks a marker is a block too, so
            // it repeats the blank gap above it below itself.
            let gap = doc
                .rows
                .last()
                .filter(|row| row.block.is_none())
                .map(|row| row.line.clone());
            while let Some(run) = hidden.get(next_hidden)
                && run.start < range.start
            {
                doc.push_hidden_marker(*run, width, styles);
                if let Some(gap) = &gap {
                    doc.push_row(gap.clone(), None);
                }
                next_hidden += 1;
            }
            doc.blocks.push(RenderedBlock {
                range,
                rows: doc.rows.len()..doc.rows.len(),
                hidden: false,
            });
        }
        let block = doc.blocks.len() - 1;
        doc.push_row(line, Some(block));
    }
    for run in &hidden[next_hidden..] {
        if doc.rows.last().is_some_and(|row| row.block.is_some()) {
            doc.push_row(Line::default(), None);
        }
        doc.push_hidden_marker(*run, width, styles);
    }
    doc
}

/// Group sorted silent lines into contiguous runs.
fn hidden_runs(silent: &[usize]) -> Vec<LineRange> {
    let mut runs: Vec<LineRange> = Vec::new();
    for &line in silent {
        let line = line as u32;
        match runs.last_mut() {
            Some(run) if run.end + 1 == line => run.end = line,
            _ => runs.push(LineRange::single(line)),
        }
    }
    runs
}

impl RenderedDocument {
    fn push_row(&mut self, line: Line<'static>, block: Option<usize>) {
        let text = line.spans.iter().map(|s| s.content.as_ref()).collect();
        if let Some(b) = block {
            self.blocks[b].rows.end = self.rows.len() + 1;
        }
        self.rows.push(RenderedRow { line, text, block });
    }

    fn push_hidden_marker(&mut self, range: LineRange, width: usize, styles: &MarkdownStyles) {
        let lines = if range.is_single() {
            format!("line {}", range.start)
        } else {
            format!("lines {}–{}", range.start, range.end)
        };
        let text = truncate_to_width(
            &format!("⋯ {lines} hidden in rendered view (:markdown for source)"),
            width,
        );
        self.blocks.push(RenderedBlock {
            range,
            rows: self.rows.len()..self.rows.len(),
            hidden: true,
        });
        let block = self.blocks.len() - 1;
        let style = styles.muted.add_modifier(Modifier::ITALIC);
        self.push_row(Line::from(Span::styled(text, style)), Some(block));
    }

    /// The block whose source range contains `line`, first in display order.
    pub fn block_containing(&self, line: u32) -> Option<usize> {
        self.blocks
            .iter()
            .position(|b| b.range.start <= line && line <= b.range.end)
    }

    /// The block a line comment stored under `line` renders after: the block
    /// containing it, else the last block starting before it, else the first.
    /// `None` only for a document with no blocks.
    pub fn anchor_block(&self, line: u32) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        self.block_containing(line).or_else(|| {
            Some(
                self.blocks
                    .iter()
                    .rposition(|b| b.range.start < line)
                    .unwrap_or(0),
            )
        })
    }

    /// The block containing `line`, or the one nearest to it, and whether the
    /// match is exact. `None` only for a document with no blocks.
    pub fn nearest_block(&self, line: u32) -> Option<(usize, bool)> {
        if let Some(b) = self.block_containing(line) {
            return Some((b, true));
        }
        let distance = |b: &RenderedBlock| {
            if line < b.range.start {
                b.range.start - line
            } else {
                line - b.range.end
            }
        };
        self.blocks
            .iter()
            .enumerate()
            .min_by_key(|(_, b)| distance(b))
            .map(|(i, _)| (i, false))
    }

    /// The first row of the first block intersecting `range`.
    pub fn first_row_intersecting(&self, range: LineRange) -> Option<usize> {
        self.blocks
            .iter()
            .find(|b| b.range.start <= range.end && range.start <= b.range.end)
            .map(|b| b.rows.start)
    }

    /// Source range of the row at `row`, `None` for a gap row.
    pub fn row_source(&self, row: usize) -> Option<LineRange> {
        let block = self.rows.get(row)?.block?;
        Some(self.blocks[block].range)
    }

    /// Whether `row` is the first row of its block.
    pub fn starts_block(&self, row: usize) -> bool {
        self.rows
            .get(row)
            .and_then(|r| r.block)
            .is_some_and(|b| self.blocks[b].rows.start == row)
    }
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4]) as &str);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Whether `path` names a Markdown document the rendered view handles.
pub fn is_markdown_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

#[cfg(test)]
mod tests;
