//! Rendered Markdown view (`:markdown`, `markdown_view = "rendered"`).
//!
//! A whole-file Markdown document (`--file`, `--all-files`) can render as laid
//! out rows instead of source lines. Rows are pre-wrapped to the unified
//! content width, one [`AnnotatedLine::MarkdownRow`] per row, and every row
//! that renders content carries the source range of its block. Comments made
//! on a row are stored against that range, so they are identical to comments
//! made in source view.
//!
//! The annotation builder (`rebuild_annotations`), the height mirror
//! (`file_render_body_height`), and the unified renderer each walk a rendered
//! file the same way: a label row in the hunk-header slot, then the rows, with
//! every line comment placed after the block [`RenderedDocument::anchor_block`]
//! picks for its stored line. They must agree row for row.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::markdown::{RenderedDocument, is_markdown_path, render_document};

/// One file's rendered document and what it was rendered from.
#[derive(Clone)]
pub(crate) struct MarkdownDocEntry {
    path: PathBuf,
    content_hash: u64,
    width: usize,
    /// Held, not just compared by address, so a freed highlighter's address
    /// can never be mistaken for the current theme's.
    highlighter: Arc<SyntaxHighlighter>,
    pub(crate) doc: Arc<RenderedDocument>,
}

/// Wrap width used before the first frame has measured the viewport.
const UNMEASURED_WIDTH: usize = 80;

/// Where the cursor sits in source terms, so a relayout can return to it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SourceAnchor {
    file_idx: usize,
    line: u32,
    rows_from_top: usize,
}

impl App {
    /// Whether `file` is a whole Markdown document the rendered view can lay
    /// out: a `.md`/`.markdown` file from a full-file source (`--file`,
    /// `--all-files`) whose single hunk starts at line 1.
    pub(crate) fn markdown_eligible(&self, file: &DiffFile) -> bool {
        self.vcs_info.vcs_type == VcsType::File
            && !matches!(self.diff_source, DiffSource::PullRequest(_))
            && is_markdown_path(file.display_path())
            && !file.is_binary
            && !file.is_too_large
            && file.hunks.len() == 1
            && file.hunks[0].new_start <= 1
    }

    /// The rendered document for `file_idx` when it renders this frame: the
    /// view is on, the layout is unified, and the document has content. The
    /// single predicate every row walk consults.
    pub(crate) fn markdown_doc(&self, file_idx: usize) -> Option<&RenderedDocument> {
        if !self.markdown_rendered || self.diff_view_mode != DiffViewMode::Unified {
            return None;
        }
        self.markdown_docs
            .get(&file_idx)
            .map(|entry| entry.doc.as_ref())
            .filter(|doc| !doc.blocks.is_empty())
    }

    /// [`Self::markdown_doc`] as a shared handle, for walks that push into
    /// `self` while reading the document.
    pub(in crate::app) fn markdown_doc_arc(
        &self,
        file_idx: usize,
    ) -> Option<Arc<RenderedDocument>> {
        self.markdown_doc(file_idx)?;
        self.markdown_docs
            .get(&file_idx)
            .map(|entry| Arc::clone(&entry.doc))
    }

    /// Whether any file in the review renders as a document right now.
    pub(crate) fn markdown_view_active(&self) -> bool {
        (0..self.diff_files.len()).any(|idx| self.markdown_doc(idx).is_some())
    }

    /// Width rendered rows wrap to: the unified content column.
    fn markdown_wrap_width(&self) -> usize {
        let viewport = self.diff_state.viewport_width;
        if viewport == 0 {
            return UNMEASURED_WIDTH;
        }
        let gutter = crate::app::unified_gutter(self.lineno_width()) as usize;
        viewport.saturating_sub(gutter).max(1)
    }

    /// Render (or reuse) the document of every eligible file. Runs at the top of
    /// `rebuild_annotations`, so the documents the row walks read always match
    /// the width and content the annotations were built for.
    pub(in crate::app) fn refresh_markdown_docs(&mut self) {
        if !self.markdown_rendered || self.diff_view_mode != DiffViewMode::Unified {
            return;
        }
        let width = self.markdown_wrap_width();
        let highlighter = self.theme.syntax_highlighter_arc();
        let mut fresh = HashMap::new();
        let mut changed: Vec<PathBuf> = Vec::new();
        for (idx, file) in self.diff_files.iter().enumerate() {
            if !self.markdown_eligible(file) {
                continue;
            }
            let path = file.display_path();
            let reusable = self.markdown_docs.get(&idx).filter(|entry| {
                entry.path == *path
                    && entry.content_hash == file.content_hash
                    && entry.width == width
                    && Arc::ptr_eq(&entry.highlighter, &highlighter)
            });
            let entry = match reusable {
                Some(entry) => entry.clone(),
                None => MarkdownDocEntry {
                    path: path.clone(),
                    content_hash: file.content_hash,
                    width,
                    highlighter: Arc::clone(&highlighter),
                    doc: Arc::new(render_document(&document_source(file), width, &self.theme)),
                },
            };
            if let Some(previous) = self
                .markdown_seen_hashes
                .insert(path.clone(), file.content_hash)
                && previous != file.content_hash
                && self
                    .session
                    .files
                    .get(path)
                    .is_some_and(|review| !review.line_comments.is_empty())
            {
                changed.push(path.clone());
            }
            fresh.insert(idx, entry);
        }
        self.markdown_docs = fresh;
        if let Some(first) = changed.first() {
            let others = changed.len() - 1;
            let subject = if others == 0 {
                first.display().to_string()
            } else {
                format!("{} and {others} more", first.display())
            };
            self.set_warning(format!(
                "{subject} changed: its comments were anchored to an earlier version; check them in source view"
            ));
        }
    }

    /// Re-render documents whose palette is stale after a theme switch. The
    /// layout does not depend on colors, so row counts — and every annotation —
    /// stay valid; only the spans change.
    pub(crate) fn restyle_markdown_docs(&mut self) {
        if self.markdown_docs.is_empty() {
            return;
        }
        let highlighter = self.theme.syntax_highlighter_arc();
        let stale: Vec<usize> = self
            .markdown_docs
            .iter()
            .filter(|(_, entry)| !Arc::ptr_eq(&entry.highlighter, &highlighter))
            .map(|(idx, _)| *idx)
            .collect();
        for idx in stale {
            let Some(file) = self.diff_files.get(idx) else {
                continue;
            };
            let source = document_source(file);
            if let Some(entry) = self.markdown_docs.get_mut(&idx) {
                entry.doc = Arc::new(render_document(&source, entry.width, &self.theme));
                entry.highlighter = Arc::clone(&highlighter);
            }
        }
    }

    /// The label shown in a rendered file's hunk-header slot.
    pub(crate) fn markdown_label(&self, file_idx: usize) -> Option<String> {
        self.markdown_doc(file_idx)?;
        let hunk = self.diff_files.get(file_idx)?.hunks.first()?;
        let last = hunk.new_start.max(1) + hunk.new_count.saturating_sub(1);
        Some(format!(
            "rendered Markdown · lines 1–{last} · :markdown for source"
        ))
    }

    /// For each block of `doc`, the stored comment lines that render after it,
    /// ascending. `extra` adds a line with no stored comment yet: the target of
    /// a comment being composed.
    pub(crate) fn markdown_comment_keys(
        doc: &RenderedDocument,
        line_comments: Option<&HashMap<u32, Vec<Comment>>>,
        extra: Option<u32>,
    ) -> BTreeMap<usize, Vec<u32>> {
        let mut lines: Vec<u32> = line_comments
            .map(|comments| comments.keys().copied().collect())
            .unwrap_or_default();
        lines.extend(extra);
        lines.sort_unstable();
        lines.dedup();
        let mut by_block: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
        for line in lines {
            if let Some(block) = doc.anchor_block(line) {
                by_block.entry(block).or_default().push(line);
            }
        }
        by_block
    }

    /// Source range of the rendered row under the cursor, `None` off a
    /// content row.
    pub fn markdown_range_at_cursor(&self) -> Option<LineRange> {
        match self.line_annotations.get(self.diff_state.cursor_line)? {
            AnnotatedLine::MarkdownRow { source, .. } => *source,
            _ => None,
        }
    }

    /// Plain text of a rendered row annotation.
    pub(crate) fn markdown_row_text(&self, file_idx: usize, row_idx: usize) -> Option<&str> {
        self.markdown_doc(file_idx)?
            .rows
            .get(row_idx)
            .map(|row| row.text.as_str())
    }

    pub fn set_markdown_rendered(&mut self, rendered: bool) {
        if self.markdown_rendered == rendered {
            self.set_message(if rendered {
                "Markdown: rendered"
            } else {
                "Markdown: source"
            });
            return;
        }
        let anchor = self.cursor_source_anchor();
        self.markdown_rendered = rendered;
        let mut switched_layout = false;
        if rendered && self.diff_view_mode == DiffViewMode::SideBySide {
            self.diff_view_mode = DiffViewMode::Unified;
            switched_layout = true;
        }
        self.rebuild_annotations();
        self.restore_source_anchor(anchor);

        let eligible = self.diff_files.iter().any(|f| self.markdown_eligible(f));
        if rendered && !eligible {
            self.set_message(
                "Markdown: rendered (applies to whole .md files opened with --file or --all-files)",
            );
        } else if switched_layout {
            self.set_message("Markdown: rendered (switched to the unified layout)");
        } else {
            self.set_message(if rendered {
                "Markdown: rendered"
            } else {
                "Markdown: source"
            });
        }
    }

    pub fn toggle_markdown_rendered(&mut self) {
        self.set_markdown_rendered(!self.markdown_rendered);
    }

    /// The source line under the cursor and how far down the viewport it sits.
    pub(crate) fn cursor_source_anchor(&self) -> Option<SourceAnchor> {
        let cursor = self.diff_state.cursor_line;
        let (file_idx, line) = match self.line_annotations.get(cursor)? {
            AnnotatedLine::MarkdownRow {
                file_idx, source, ..
            } => (*file_idx, source.map(|r| r.start)?),
            AnnotatedLine::DiffLine {
                file_idx,
                new_lineno,
                old_lineno,
                ..
            } => (*file_idx, new_lineno.or(*old_lineno)?),
            AnnotatedLine::LineComment { file_idx, line, .. } => (*file_idx, *line),
            _ => return None,
        };
        Some(SourceAnchor {
            file_idx,
            line,
            rows_from_top: cursor.saturating_sub(self.diff_state.scroll_offset),
        })
    }

    /// Put the cursor back on the row covering `anchor`'s source line, at the
    /// same distance from the top of the viewport.
    pub(crate) fn restore_source_anchor(&mut self, anchor: Option<SourceAnchor>) {
        let Some(anchor) = anchor else {
            return;
        };
        let found = self.line_annotations.iter().position(|ann| match ann {
            AnnotatedLine::MarkdownRow {
                file_idx,
                source: Some(range),
                ..
            } => {
                *file_idx == anchor.file_idx
                    && range.start <= anchor.line
                    && anchor.line <= range.end
            }
            AnnotatedLine::DiffLine {
                file_idx,
                new_lineno,
                old_lineno,
                ..
            } => *file_idx == anchor.file_idx && new_lineno.or(*old_lineno) == Some(anchor.line),
            _ => false,
        });
        let Some(idx) = found else {
            return;
        };
        self.diff_state.cursor_line = idx;
        self.diff_state.scroll_offset = idx
            .saturating_sub(anchor.rows_from_top)
            .min(self.max_scroll_offset());
        self.ensure_cursor_visible();
        self.update_current_file_from_cursor();
    }

    /// Visible line comments of a rendered file: every one renders, after the
    /// block its stored line anchors to.
    pub(in crate::app) fn markdown_comment_rows(
        &self,
        path: &Path,
        commit_set: Option<&HashSet<String>>,
    ) -> usize {
        let Some(review) = self.session.files.get(path) else {
            return 0;
        };
        review
            .line_comments
            .values()
            .flatten()
            .filter(|comment| Self::comment_visible_with(comment, commit_set))
            .map(|comment| Self::comment_display_lines(comment, self.diff_state.viewport_width))
            .sum()
    }
}

/// The document text of a whole-file source: its single hunk's lines.
fn document_source(file: &DiffFile) -> String {
    let mut source = String::new();
    if let Some(hunk) = file.hunks.first() {
        for (i, line) in hunk.lines.iter().enumerate() {
            if i > 0 {
                source.push('\n');
            }
            source.push_str(&line.content);
        }
    }
    source
}
