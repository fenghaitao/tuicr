# Rendered Markdown review in tuicr

Status: Proposed  
Date: 2026-10-19  
Audience: tuicr implementation contributors and Conductor/AutoDML maintainers

## Decision

Add a rendered Markdown view to tuicr, adapting the renderer and source mapping from herdr-reviewr. Start with complete-document annotation through `tuicr --file`; retain tuicr's existing source-code diff review, saved review sessions, and JSON comment interface.

The reviewer must be able to read a rendered specification, select a paragraph or table row, and leave a comment anchored to the corresponding lines in the original `.md` file. Switching to source view must show the same comment. Rendering is a presentation mode, not a new document or comment format.

Rendered Markdown diffs are a subsequent phase. They require explicit handling of deleted content and changes that disappear when rendered.

## Problem and workflow

Conductor/AutoDML work requires human review of both specifications and implementation. Specifications may need approval before implementation, including sections that have no Git changes. Implementation review happens at task and iteration boundaries, and findings need to survive interruptions and be consumable by the workflow.

The inspected AutoDML workflow records an iteration's starting commit as `base_0`, packages the iteration diff, writes `final-review.md`, and routes findings into a fix and re-review cycle. A terminal application's latest agent turn is not an adequate substitute for those task and iteration boundaries.

Tuicr provides persisted comments, file/hunk review marks, commit-range review, and a JSON review CLI. Its installed version, 0.27.0, was tested with a Markdown file containing headings, emphasis, a table, lists, and fenced code. It displayed styled Markdown source, retaining markup delimiters. Inspection of its current source and controls found no rendered-document preview.

Herdr-reviewr supports rendered Markdown and comments made directly in that view. Its renderer maps display content back to source ranges. Its existing tests cover equivalence between source and rendered comments, table-row mappings, resizing, and comments remaining visible across view changes. Its in-memory comment lifecycle is less suitable for the durable feedback artifacts required here.

## Goals

- Read complete Markdown specifications in rendered or source form.
- Comment directly from the rendered view, using source-line anchors.
- Preserve comments and review progress through tuicr's existing session store.
- Keep comments available through `tuicr review comments` without requiring Herdr.
- Retain tuicr's existing code review behavior and source-mode precision.
- Reuse reviewr's implementation and regression cases where they fit.

## Initial scope

The first release supports `.md` and `.markdown` documents opened through `--file`, including Markdown files within a directory review. Other files in the same review retain their existing presentation.

The renderer supports headings, paragraphs, emphasis, links as text, lists, task lists, blockquotes, fenced code with syntax highlighting, and tables. Table rows retain individual source mappings. Long content wraps to the available width. If a table cannot be represented legibly, show its source with an explicit fallback indication.

The first release does not implement rendered Git diffs, MDX component execution, HTML/browser rendering, Mermaid execution, image loading, or automatic semantic reanchoring after source edits. Existing diff review remains available for Markdown source changes as well as code.

## User experience

### Specification review

Existing invocation:

```bash
tuicr --file specs/design.md
```

Proposed additions:

- `:markdown` toggles the focused document between source and rendered views.
- `:set markdown=source` and `:set markdown=rendered` choose an explicit mode.
- A `markdown_view = "source"` configuration setting preserves the current default; users may select `"rendered"`.
- A visible label identifies the current mode and the source range under the cursor.

These commands and the configuration setting are proposals, not current tuicr features. Keep `m` and `M` available for tuicr's existing comment navigation.

In rendered mode, the reviewer moves to a paragraph, presses `c`, writes a comment, and saves it through the existing composer. Visual selection may span several rendered blocks. Tuicr records the corresponding source range, not terminal row numbers.

For example, a paragraph occupying source lines 40–43 may occupy two or six screen rows depending on the pane width. A comment on that paragraph continues to reference `spec.md:40–43` at either width.

Switching to source view reveals those source lines and the existing comment. Source view remains the way to choose a narrower source-line range.

### Code review

Existing commands remain unchanged:

```bash
tuicr -w
tuicr -r BASE_COMMIT..HEAD
```

For AutoDML, review a complete spec and the relevant implementation scope in separate panes or sessions. The workflow selects the recorded task/iteration baseline and reviewed paths. Do not infer review scope from pane activity or replace the existing review package with an arbitrary branch diff.

### Comments and persistence

Comments are separate review data; they do not modify the Markdown document. Saving, reopening, comment types, authors, exports, and review marks continue to use tuicr's existing behavior.

The renderer must not change session identity. Source and rendered modes are two views of the same review target, not separate sessions. Creating a comment in one view and editing it in the other updates the same comment ID.

## Architecture

### 1. Adapt the Markdown renderer

Both inspected projects use Rust, ratatui 0.30, pulldown-cmark 0.13.4, syntect, and unicode-width. The parser and terminal text types already align. This reduces dependency work but does not make the renderer a drop-in module.

Adapt reviewr's document renderer behind a small tuicr-owned interface. Replace its application-specific palette and highlighter dependencies with adapters to tuicr's theme and syntax highlighter. Do not import reviewr's application state, Herdr control code, or comment store.

Tuicr's existing `src/syntax/cmark.rs` styles Markdown comment bodies while preserving source text. Keep that behavior separate from document layout; adding a preview must not unexpectedly change comment composition or PR-description rendering.

Proposed conceptual output:

```rust
struct RenderedDocument {
    rows: Vec<RenderedRow>,
    hidden_source_ranges: Vec<SourceRange>,
}

struct RenderedRow {
    line: ratatui::text::Line<'static>,
    source: Option<SourceRange>,
    block_id: BlockId,
}

struct SourceRange {
    start: u32,
    end: u32,
}
```

Names are illustrative. `SourceRange` is inclusive and one-based, matching tuicr's existing comment ranges. `BlockId` is a transient identity within one source snapshot, not a permanent anchor across edits.

### 2. Introduce a rendered-document annotation

Tuicr currently uses `AnnotatedLine` to connect displayed content to cursor movement, selection, comments, and source locations. Add a document-row variant containing file identity, rendered row identity, and an optional source range. Avoid pretending a rendered paragraph is an ordinary single-line diff row.

Route single-line comments, visual selections, comment navigation, and editor opening through shared source-location helpers. Existing diff annotations continue to resolve their old/new line locations as before; document annotations resolve a new-side source range.

Keep rendered-mode dispatch limited to supported full-file review targets in the first release. Opening a Markdown Git diff must not silently substitute its complete current document and hide deleted content.

### 3. Define source mapping explicitly

| Rendered content | Comment anchor |
| --- | --- |
| Heading or paragraph | Whole source block |
| List item | Item's source range, excluding nested sibling units |
| Table row | Source row that produced it |
| Code block | Code block's source range, including its fences in the first release |
| Wrapped continuation | Same source range as the block it continues |
| Synthetic padding or decoration | No independent anchor |
| Content hidden by rendering | Explicit source-only marker with its source range |

For a selection spanning multiple blocks, use the smallest contiguous source range containing the selected anchorable blocks. Blank lines between those blocks are included. Display that range before submission so the reviewer can switch to source for finer control.

A rendered paragraph maps to a source range; it does not support character-precise comments. File-level and review-level comments remain available when a location is inappropriate.

Never silently discard content that renders nothing. Use reviewr's source-map approach to identify omitted material, such as HTML comments or reference definitions, and provide an actionable marker that returns to source view.

### 4. Reuse the existing comment model

Convert the resolved location to tuicr's existing `CommentTarget::Line` or `CommentTarget::LineRange`, including file path and `LineSide::New`, and pass it through the existing add-comment path.

The MVP needs no new persisted comment fields and no changes to the JSON export contract. Presentation mode, rendered row numbers, and terminal width must not enter saved comment anchors. File and review-level targets remain unchanged.

Comments created on a narrower range in source mode must remain discoverable in rendered mode. A block intersecting a comment range exposes the comment without rewriting its original source range or duplicating its card for every wrapped row.

### 5. Handle resize, navigation, and reload

Cache rendered output by document snapshot, width, theme, and renderer options. Do not parse the whole document on every frame. Invalidate the cache on source reload, width changes, or relevant theme/options changes.

Before resizing or switching views, capture a source location and its viewport-relative position. Rebuild the display, then locate the corresponding block. Preserve comments by their source anchors and preserve an active comment draft's original target, regardless of reflow.

Search, keyboard selection, mouse hit-testing, scrolling, editor opening, and comment jumps must all resolve against the active display map. The MVP uses rendered text for rendered-view search; source view remains available to search markup that is omitted from the preview. Copying a visual selection copies visible rendered text, while comment creation resolves source ranges.

This feature does not add semantic reanchoring across edits. AutoDML should pause edits while a human reviews a snapshot. If the source changes during a review, invalidate the rendered map, preserve stored comments, and warn that anchors require revalidation. Never present old coordinates as newly verified anchors. A view toggle or resize alone must not produce such a warning.

## Conductor/AutoDML integration

The Markdown feature is useful standalone. A Conductor adapter is a separate integration task; it is not a prerequisite for shipping the renderer.

Proposed workflow:

1. At the existing specification review boundary, open the complete spec with tuicr.
2. Let the user write comments in rendered or source view.
3. Discover and record the exact session ID/path through `tuicr review list`.
4. Read comments using `tuicr review comments --session SESSION`.
5. Save exported feedback alongside the workflow's artifacts, together with target identity and source snapshot information.
6. Supply spec feedback to the spec revision step; supply implementation feedback to the appropriate fix step.

Example of the existing export mechanism:

```bash
tuicr review comments --session SESSION > human-spec-review.json
```

The adapter must record the repository or standalone document path, reviewed commit where applicable, content digest for a mutable spec, review scope, and completion decision. Those are proposed workflow metadata, not new tuicr comment fields.

Do not infer approval from zero comments or a CLI exit code. Review progress is useful evidence, but the existing Conductor human decision remains the acceptance boundary. Also do not make a source-code TUI the authority for DML/Simics correctness: existing contract checks, builds, tests, and automated review remain part of AutoDML's validation.

The deployed AutoDML setup contains a VP repository and a nested workflow repository. Resolve each review against the actual owning repository or explicit spec path; do not assume all specs and production files share one Git root.

## Reuse and attribution

Adapt these parts of herdr-reviewr:

- `src/markdown.rs`: document layout, tables, wrapping, and source metadata.
- `src/rendered.rs`: display state and row identity concepts.
- `src/app.rs`: rendered selection to source-anchor behavior.
- `tests/app_flow.rs` and renderer tests: regression cases for comment equivalence, resizing, tables, and source/rendered switching.

Reviewr's `src/marks.rs` handles change ownership and markers in rendered diffs. Study it for phase two; do not pull the full diff machinery into the full-document MVP.

The renderer is application code, not a published independent library. Prefer an attributed vendored module with tuicr-specific adapters over a runtime dependency on reviewr. Record the imported upstream revision and local adaptations so subsequent fixes can be compared deliberately.

Both inspected repositories declare MIT licenses. Retain reviewr's copyright and license notice with copied or substantially adapted code and include attribution for adapted tests. Check any additional assets separately; no themes or binary syntax assets need to be copied for the proposed adapter.

## Delivery sequence

| Stage | Deliverable | Exit condition |
| --- | --- | --- |
| 1. Renderer | Adapted document renderer and source metadata | Layout and mapping fixtures pass |
| 2. Annotation | Source/rendered toggle and comment creation in `--file` mode | Comments retain source anchors in either view |
| 3. Persistence and interaction | Session reopening, CLI export, search, mouse, resize, editor jumps | Complete review loop works in a terminal |
| 4. AutoDML adapter | Explicit session selection and exported review artifacts | Feedback reaches the intended workflow step |
| 5. Rendered diffs | New-side rendering, deletion/hidden-change markers, scope handling | Every change remains inspectable and commentable |

Stage four can ship separately from the tuicr contribution. Stage five should be a separate design/implementation increment rather than a requirement for the first usable spec reviewer.

## Verification and acceptance

The central invariant is: **the same source selection produces the same persisted comment target regardless of presentation width or view mode.**

Required checks:

- Renderer fixtures cover headings, multi-line paragraphs, nested lists, tables, Unicode, code fences, and content omitted from rendering.
- Each rendered selectable block has a valid source range; decoration cannot create a misleading anchor.
- Equivalent source/rendered comments have the same path, side, range, content, and author.
- Selecting a paragraph or table row, resizing, and saving does not change the pending target.
- Source-created comments remain visible and editable after switching to rendered mode, including narrow ranges within a larger rendered block.
- Saving and reopening preserves comments, IDs, types, and file review marks.
- `tuicr review comments` produces the existing JSON shape for rendered-created comments.
- Source reload invalidates the display map and warns about unverified old anchors.
- Markdown code fences are highlighted but not executed; links/images require no network activity for rendering.
- Existing source-code diff, visual-selection, comment navigation, persistence, and export tests continue to pass.
- A terminal smoke test covers opening a real spec, creating a rendered comment, toggling source, resizing, saving, reopening, and reading the comment through the CLI. Include a narrow pane and a wide pane, and keyboard and mouse selection.

Use existing renderer/performance tests to check that idle frames reuse cached documents. Add measurements before setting a new latency budget; no performance claims are made by this design.

## Risks and choices for implementation

- Reviewr's renderer depends on its own palette and highlighter. Keep those dependencies behind a small adapter rather than importing unrelated application modules.
- Tuicr's annotation and navigation logic frequently assumes one source line per annotation. Adding a rendered range requires auditing those consumers, not just changing drawing code.
- Rendered blocks are coarser than source lines. Make the source range visible and retain immediate source switching.
- Documentation on a moving branch can differ from a released binary. Pin the reference implementation and test the actual tuicr revision being changed.
- Existing tuicr session identity and file-mode persistence need regression coverage before promising long-lived AutoDML review artifacts.

Proposed defaults are source view, block-level rendered anchors, and a command-based toggle. Confirm these against tuicr's current contribution conventions during implementation. A reusable Markdown-rendering crate may be worth discussing upstream later, but is not required for this port.

## References and inspected revisions

- [tuicr source, revision `475846bf71b44d9853d6d668e1a7c90c500f6f4d`](https://github.com/agavra/tuicr/tree/475846bf71b44d9853d6d668e1a7c90c500f6f4d), package version 0.27.0.
- [tuicr file backend](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/vcs/file.rs).
- [tuicr annotation model](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/app/mod.rs), [selection](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/app/visual.rs), and [comment creation](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/src/app/comments.rs).
- [tuicr review CLI](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/docs/REVIEW_CLI.md).
- [herdr-reviewr source, revision `4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8`](https://github.com/persiyanov/herdr-reviewr/tree/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8), package version 0.47.0.
- [reviewr Markdown renderer](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/src/markdown.rs), [rendered state](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/src/rendered.rs), and [mapping/interaction tests](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/tests/app_flow.rs).
- [tuicr license](https://github.com/agavra/tuicr/blob/475846bf71b44d9853d6d668e1a7c90c500f6f4d/LICENSE) and [reviewr license](https://github.com/persiyanov/herdr-reviewr/blob/4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8/LICENSE).
- Local AutoDML evidence: `/home/hfeng1/conductor-autodml/README.md`, `phases/shared/implement.yaml` (`base_0`, `final_package`, `final_review`, and fix/re-review routing), and `lib/implement/review-package.py`.

This document records source inspection and the manual tuicr Markdown display check. It does not claim that the proposed feature or a Conductor adapter has been implemented or tested.
