# Rendered Markdown review in tuicr

Status: Implemented (stages 1–3)  
Date: 2026-10-19  
Audience: tuicr implementation contributors

Workflow integration for Conductor/AutoDML is a separate note:
[tuicr-rendered-markdown-autodml.md](tuicr-rendered-markdown-autodml.md).

## Decision

Add a rendered Markdown view to tuicr, adapting the renderer and source mapping from herdr-reviewr. Start with complete-document annotation through `tuicr --file` and `tuicr --all-files`; retain tuicr's existing source-code diff review, saved review sessions, and JSON comment interface.

The reviewer must be able to read a rendered specification, select a paragraph or table row, and leave a comment anchored to the corresponding lines in the original `.md` file. Switching to source view must show the same comment. Rendering is a presentation mode, not a new document or comment format.

Rendered Markdown diffs are a subsequent phase. They require explicit handling of deleted content and changes that disappear when rendered.

## Problem

Specifications need human review as complete documents, including sections that have no Git changes, and the findings need to survive interruptions and be consumable by tooling.

Tuicr provides persisted comments, file/hunk review marks, commit-range review, and a JSON review CLI. Version 0.27.0 was tested with a Markdown file containing headings, emphasis, a table, lists, and fenced code. It displayed styled Markdown source, retaining markup delimiters. Inspection of its source and controls found no rendered-document preview.

Herdr-reviewr supports rendered Markdown and comments made directly in that view. Its renderer maps display content back to source ranges. Its tests cover equivalence between source and rendered comments, table-row mappings, resizing, and comments remaining visible across view changes. Its in-memory comment lifecycle is less suitable for durable feedback; tuicr's session store already provides that.

## Goals

- Read complete Markdown specifications in rendered or source form.
- Comment directly from the rendered view, using source-line anchors.
- Preserve comments and review progress through tuicr's existing session store.
- Keep comments available through `tuicr review comments` without requiring Herdr.
- Retain tuicr's existing code review behavior and source-mode precision.
- Reuse reviewr's implementation and regression cases where they fit.

## Initial scope

Rendered view applies to `.md` and `.markdown` files whose review target is the complete file: `--file <path>`, `--file <dir>` (directory review), and `--all-files`. In code these are the `FileBackend` sources (`VcsType::File`), where each file is a single hunk starting at line 1. Other files in the same review keep their existing presentation, as do Markdown files in Git diffs, commit ranges, and pull requests.

The renderer supports headings, paragraphs, emphasis, links as text, lists, task lists, blockquotes, fenced code with syntax highlighting, tables, thematic breaks, and `<details>` blocks (always shown expanded). Table rows retain individual source mappings. Long content wraps to the available width. If a table cannot be represented legibly at any column floor, it shows its source text instead, dimmed.

The first release does not implement rendered Git diffs, MDX component execution, HTML/browser rendering, Mermaid execution (a fence collapses to a `⧉ mermaid <kind>` placeholder), image loading (an image collapses to `⧉ alt text`), clickable links, or automatic semantic reanchoring after source edits. Existing diff review remains available for Markdown source changes as well as code.

## User experience

### Specification review

```bash
tuicr --file specs/design.md
tuicr --file specs/          # every file in the directory
tuicr --all-files            # every tracked file in the repository
```

Commands, following tuicr's boolean `:set` convention (`:set X`, `:set noX`, `:set X!`, plus a short alias):

| Command | Effect |
| --- | --- |
| `:markdown` / `:set markdown!` | Toggle rendered view |
| `:set markdown` | Rendered view |
| `:set nomarkdown` | Source view |

The mode is global: it applies to every eligible file in the review, not per file. Like every other `:set` toggle it lasts for the session and never writes `config.toml`.

Configuration: `markdown_view = "source"` (the default) or `"rendered"` chooses the startup mode.

`m` and `M` keep their existing comment-navigation meaning. No single-key binding is added; tuicr reserves single strokes for vim motions.

Labels:

- Each rendered file's hunk-header row reads `rendered Markdown · lines 1–N · :markdown for source`, replacing the `@@ -0,0 +1,N @@` header.
- The header bar shows a `MARKDOWN` chip while rendered view is on, followed by the source range under the cursor (`MARKDOWN · L40–43`).
- The gutter shows the first source line of each block on that block's first row; continuation and gap rows have a blank gutter. With `relative_line_numbers`, distances count rendered rows, matching the existing behaviour for source rows.

In rendered mode the reviewer moves to a paragraph, presses `c`, writes a comment, and saves it through the existing composer. `v` starts a visual selection that may span several rendered blocks. Tuicr records the corresponding source range, not terminal row numbers, and the composer header and `VISUAL L40-43` status show that range before saving.

A paragraph occupying source lines 40–43 may occupy two or six screen rows depending on the pane width. A comment on that paragraph references `spec.md:40-43` at either width.

Switching to source view reveals those source lines and the existing comment. Source view remains the way to choose a narrower source-line range.

### Side-by-side layout

Rendered view is a single-column layout and applies only in the unified diff layout. Turning it on while side-by-side is active switches to unified and says so. Switching to side-by-side with `:diff` while rendered view is on shows Markdown files as source in side-by-side; returning to unified shows them rendered again. `--all-files` is already unified-only.

### Code review

Existing commands remain unchanged:

```bash
tuicr -w
tuicr -r BASE_COMMIT..HEAD
```

### Comments and persistence

Comments are separate review data; they do not modify the Markdown document. Saving, reopening, comment types, authors, exports, and review marks continue to use tuicr's existing behavior.

The renderer does not change session identity. Source and rendered modes are two views of the same review target, not separate sessions. Creating a comment in one view and editing it in the other updates the same comment ID.

## Architecture

### 1. Adapt the Markdown renderer

Both projects use Rust, ratatui 0.30, pulldown-cmark 0.13.4, syntect, and unicode-width. The parser and terminal text types already align. This reduces dependency work but does not make the renderer a drop-in module.

The renderer lives in `src/markdown/` as an attributed, vendored adaptation of reviewr's `src/markdown.rs` (see Reuse and attribution). It sits behind a small tuicr-owned interface:

- Styles come from tuicr's existing `MarkdownPalette` (`src/syntax/cmark.rs`), which is resolved from the active syntect theme, plus tuicr `Theme` colors for prose, borders, and muted text. The preview therefore matches the colors source view uses for the same constructs.
- Fenced code is highlighted with tuicr's `SyntaxHighlighter`, resolving the fence's language token the same way comment bodies do.
- Nothing from reviewr's application state, Herdr control code, palette, highlighter, or comment store is imported.

`cmark::highlight` keeps styling comment bodies while preserving source text. Document layout is a separate path; adding the preview does not change comment composition or PR-description rendering.

Output:

```rust
pub struct RenderedDocument {
    pub rows: Vec<RenderedRow>,
    pub blocks: Vec<RenderedBlock>,
}

pub struct RenderedRow {
    pub line: ratatui::text::Line<'static>,
    pub text: String,             // plain text, for search, copy, and mouse hit-testing
    pub block: Option<usize>,     // None for the blank gap rows between blocks
}

pub struct RenderedBlock {
    pub range: LineRange,         // inclusive, one-based source lines
    pub rows: std::ops::Range<usize>,
    pub hidden: bool,             // a source-only marker for content that renders nothing
}
```

`LineRange` is tuicr's existing comment range type. Block indices are transient identities within one source snapshot, not anchors across edits.

### 2. Row model: pre-wrapped rows

The renderer wraps to a width, so rendered rows are pre-wrapped, the same way comment boxes already are. One `AnnotatedLine::MarkdownRow { file_idx, row_idx, source }` annotation is emitted per rendered row, with `source: Option<LineRange>` copied from the row's block.

- The wrap width is the unified content width: `viewport_width − unified_gutter(lineno_width)`. Rows use the unified gutter layout (cursor indicator, line-number field, two-column marker), so mouse hit-testing, column-to-character mapping, and the comment bar column need no special cases.
- A width change already rebuilds annotations through `App::sync_viewport_width`; that rebuild re-renders the documents at the new width.
- `annotation_row_height` measures `MarkdownRow` with the same wrap pass as diff content rows. Because rows already fit, the result is 1 except for grapheme-width edge cases, and both sides agree by construction.
- `:set wrap!` does not change the rendered layout, because rows already fit the viewport.
- `App::file_render_height` counts rendered files as: header + label row + rendered rows + every visible line comment of the file. It must stay in lockstep with the annotation builder and the unified renderer, like every other row kind.

### 3. Define source mapping explicitly

| Rendered content | Comment anchor |
| --- | --- |
| Heading or paragraph | Whole source block |
| List item | The item's own lines, excluding nested child lists, which are their own blocks |
| Table row | Source row that produced it |
| Table header rule | The delimiter row (`\|---\|`) |
| Code block | Code block's source range, including its fences |
| Wrapped continuation | Same source range as the block it continues |
| Gap row between blocks | No anchor |
| Content hidden by rendering | Source-only marker row with its source range |

A block's range ends at its last non-blank line.

For a selection spanning multiple blocks, use the smallest contiguous source range containing the selected anchorable blocks. Blank lines between those blocks are included. The selection's range is shown before submission so the reviewer can switch to source for finer control.

A rendered paragraph maps to a source range; it does not support character-precise comments. File-level and review-level comments remain available when a location is inappropriate.

Content that renders nothing is never discarded silently. HTML comments, link reference definitions, and other source lines that produce no output become a marker row: `⋯ lines 12–14 hidden in rendered view (:markdown for source)`. It is placed in source order, is selectable and commentable, and anchors to its range.

### 4. Reuse the existing comment model

The resolved location becomes tuicr's existing `comment_line` / `comment_line_range` state: `CommentTarget::Line` for a one-line block, `CommentTarget::LineRange` otherwise, always `LineSide::New`. It goes through the existing `save_comment` → `add_comment_to_session` path.

No new persisted comment fields and no change to the JSON export contract. Presentation mode, rendered row numbers, and terminal width never enter saved comment anchors.

### 5. Comment card placement

Line comments are stored under their range's end line (`line_comments: HashMap<u32, Vec<Comment>>`), and source view draws the card after that line. Rendered view places each stored key `L` as follows:

1. The card renders after the last row of the block whose range contains `L`.
2. If no block contains `L` (a blank line, or a line before the first block), it attaches to the last block starting before `L`, or to the first block if none does.
3. Keys that land on the same block render in ascending line order, each key's comments in stored order, followed by the composer when a new comment targets that key.

So every comment of the file is shown exactly once in rendered view, whatever its range. A source-made comment on lines 10–30 spanning three blocks appears once, after the block containing line 30. Its range bar covers the rendered rows from the first block intersecting line 10 down to the card, rather than one row per source line as in source view.

### 6. Navigation, search, and source-line features

These resolve against the active display map:

- `--line N` and `go_to_source_line`: land on the first row of the block containing `N`, or the nearest block.
- `e` (open in `$EDITOR`): opens at the first source line of the block under the cursor.
- `/` search: matches rendered row text. Source view remains available to search markup the preview omits.
- Visual-mode `y`: copies the rendered text of the selection (tuicr's existing visual copy); comment creation resolves source ranges.
- `R` (hunk reviewed): a full-file source is a single hunk, so `R` on any rendered row marks that hunk, and a reviewed hunk collapses to its label row as in source view.
- Mouse click, drag selection, scrolling, and comment navigation work as they do for source rows.

### 7. Resize, toggle, and reload

Documents are cached per file by source content hash, wrap width, and syntax-highlighter identity. Nothing is parsed on idle frames. A theme switch re-renders with the new palette; the layout does not depend on colors, so row counts stay the same.

Before toggling the view or resizing, tuicr captures the source line under the cursor and its viewport-relative position, rebuilds, and returns to the row covering that line.

This feature does not add semantic reanchoring across edits. If a reload (`:e`, the diff watcher) brings different content for a rendered file that has comments, tuicr warns that its comment anchors were made against an earlier version. Stored comments are kept as they are. This relies on tuicr's existing file content hashes, which already drive reload invalidation of review marks. A view toggle or resize alone never produces the warning.

## Reuse and attribution

Adapted from herdr-reviewr:

- `src/markdown.rs`: document layout, tables, wrapping, and source metadata.
- `src/rendered.rs`: block/unit identity concepts used for the block index.
- Renderer tests: regression cases for tables, wrapping, nesting, hidden content, and source mapping.

Reviewr's `src/marks.rs` handles change ownership and markers in rendered diffs. It is reference material for phase two; the full diff machinery is not part of the full-document MVP.

The renderer is application code, not a published library, so it is vendored with tuicr-specific adapters rather than added as a runtime dependency. The module header records the upstream revision and lists local adaptations, so later upstream fixes can be compared deliberately.

Both repositories use the MIT license. `src/markdown/LICENSE-reviewr` keeps reviewr's copyright and license notice for the adapted code and tests. No themes or binary syntax assets are copied.

## Delivery sequence

| Stage | Deliverable | Exit condition |
| --- | --- | --- |
| 1. Renderer | Adapted document renderer and source metadata | Layout and mapping fixtures pass |
| 2. Annotation | Rendered toggle and comment creation in full-file modes | Comments keep source anchors in either view |
| 3. Persistence and interaction | Session reopening, CLI export, search, mouse, resize, editor jumps, `--line`, docs | Complete review loop works in a terminal |
| 4. Rendered diffs | New-side rendering, deletion/hidden-change markers, scope handling | Every change remains inspectable and commentable |

Stage 4 is a separate design/implementation increment, not a requirement for the first usable spec reviewer.

User-facing documentation ships with stage 3, per AGENTS.md:

- `README.md`: feature bullet and the `:markdown` command.
- `docs/KEYBINDINGS.md`: the `:markdown` / `:set markdown` rows and prose.
- `docs/CONFIG.md`: the `markdown_view` option row and full-example line.
- `src/ui/help_popup.rs`: the command entries.
- `AGENTS.md`: the `src/markdown/` module, the `MarkdownRow` annotation, and the lockstep invariant.

## Verification and acceptance

The central invariant is: **the same source selection produces the same persisted comment target regardless of presentation width or view mode.**

Required checks:

- Renderer fixtures cover headings, multi-line paragraphs, nested lists, tables (including the header rule and the source fallback), Unicode, code fences, and content omitted from rendering.
- Each rendered selectable block has a valid source range; gap rows and decoration have none.
- Equivalent source and rendered comments have the same path, side, range, content, and author.
- Selecting a paragraph or table row at two widths produces the same pending target.
- Source-created comments remain visible after switching to rendered mode, including narrow ranges within a larger rendered block and ranges ending on blank lines.
- `annotation_row_height`, `file_render_height`, and the unified renderer agree on row counts for rendered files.
- Saving and reopening preserves comments, IDs, types, and file review marks.
- `tuicr review comments` produces the existing JSON shape for rendered-created comments.
- Reloading changed content of a commented file warns; toggling or resizing does not.
- Markdown code fences are highlighted but not executed; links and images cause no network activity.
- Existing source-code diff, visual-selection, comment navigation, persistence, and export tests continue to pass.
- A terminal smoke test covers opening a real spec, creating a rendered comment, toggling source, resizing, saving, reopening, and reading the comment through the CLI, with a narrow and a wide pane.

No performance budget is set by this design; documents render once per content/width change, and idle frames reuse the cache.

## Risks

- Tuicr's annotation, height, and renderer code paths each walk files independently and must agree row for row. `MarkdownRow` adds one more row kind to all three; tests assert the counts agree.
- Rendered blocks are coarser than source lines. The source range is always shown and source view is one command away.
- A vendored renderer drifts from upstream. The recorded revision and adaptation list keep comparisons deliberate.

A reusable Markdown-rendering crate may be worth discussing with reviewr upstream later, but is not required for this port.

## References and inspected revisions

- [tuicr source, revision `475846bf71b44d9853d6d668e1a7c90c500f6f4d`](https://github.com/agavra/tuicr/tree/475846bf71b44d9853d6d668e1a7c90c500f6f4d), package version 0.27.0. This fork (`fenghaitao/tuicr`) branches from that revision.
- [tuicr file backend](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/vcs/file.rs).
- [tuicr annotation model](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/app/mod.rs), [selection](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/app/visual.rs), and [comment creation](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/app/comments.rs).
- [tuicr review CLI](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/docs/REVIEW_CLI.md).
- [herdr-reviewr source, revision `4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8`](https://github.com/persiyanov/herdr-reviewr/tree/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8), package version 0.47.0.
- [reviewr Markdown renderer](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/src/markdown.rs), [rendered state](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/src/rendered.rs), and [mapping/interaction tests](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/tests/app_flow.rs).
- [tuicr license](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/LICENSE) and [reviewr license](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/LICENSE), both MIT.
