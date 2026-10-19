//! Rendered Markdown view: the central invariant is that the same source
//! selection produces the same persisted comment target regardless of view
//! mode or width, and that the annotation builder, the height mirror, and the
//! unified renderer agree row for row on a rendered file.

use crate::app::*;
use crate::input::Action;
use crate::model::{DiffFile, DiffHunk, DiffLine, FileStatus, LineOrigin};
use crate::vcs::traits::{VcsBackend, VcsInfo, VcsType};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::PathBuf;

struct StubVcs(VcsInfo);
impl VcsBackend for StubVcs {
    fn info(&self) -> &VcsInfo {
        &self.0
    }
    fn get_working_tree_diff(
        &self,
        _hl: &crate::syntax::SyntaxHighlighter,
    ) -> crate::error::Result<Vec<DiffFile>> {
        Ok(Vec::new())
    }
    fn fetch_context_lines(
        &self,
        _path: &std::path::Path,
        _status: FileStatus,
        _ref_commit: Option<&str>,
        _start: u32,
        _end: u32,
    ) -> crate::error::Result<Vec<DiffLine>> {
        Ok(Vec::new())
    }
    fn file_line_count(
        &self,
        _path: &std::path::Path,
        _status: FileStatus,
        _ref_commit: Option<&str>,
    ) -> crate::error::Result<u32> {
        Ok(0)
    }
}

const SPEC: &[&str] = &[
    "# Spec title",                // 1
    "",                            // 2
    "A paragraph on line three",   // 3
    "that continues on line four", // 4
    "and ends on five.",           // 5
    "",                            // 6
    "| k | v |",                   // 7
    "|---|---|",                   // 8
    "| row | value |",             // 9
    "",                            // 10
    "<!-- reviewer note -->",      // 11
    "",                            // 12
    "Final **bold** words.",       // 13
];

/// A `--file`-shaped document: one addition hunk from line 1.
fn file(path: &str, contents: &[&str]) -> DiffFile {
    let lines = contents
        .iter()
        .enumerate()
        .map(|(idx, content)| DiffLine {
            origin: LineOrigin::Addition,
            content: content.to_string(),
            old_lineno: None,
            new_lineno: Some(idx as u32 + 1),
            highlighted_spans: None,
        })
        .collect::<Vec<_>>();
    let hunks = vec![DiffHunk {
        header: format!("@@ -0,0 +1,{} @@", contents.len()),
        lines,
        old_start: 0,
        old_count: 0,
        new_start: 1,
        new_count: contents.len() as u32,
    }];
    let content_hash = DiffFile::compute_content_hash(&hunks);
    DiffFile {
        old_path: None,
        new_path: Some(PathBuf::from(path)),
        status: FileStatus::Added,
        hunks,
        is_binary: false,
        is_too_large: false,
        is_commit_message: false,
        content_hash,
    }
}

fn vcs_info(vcs_type: VcsType) -> VcsInfo {
    VcsInfo {
        root_path: PathBuf::from("/tmp/specs"),
        head_commit: "head".into(),
        branch_name: None,
        vcs_type,
    }
}

fn app_with(files: Vec<DiffFile>, vcs_type: VcsType) -> App {
    let info = vcs_info(vcs_type);
    let session = ReviewSession::new(
        info.root_path.clone(),
        info.head_commit.clone(),
        info.branch_name.clone(),
        SessionDiffSource::WorkingTree,
    );
    app_with_session(files, vcs_type, session)
}

fn app_with_session(files: Vec<DiffFile>, vcs_type: VcsType, session: ReviewSession) -> App {
    let vcs_info = vcs_info(vcs_type);
    let mut app = App::build(
        Box::new(StubVcs(vcs_info.clone())),
        vcs_info,
        crate::theme::Theme::dark(),
        None,
        false,
        files,
        session,
        DiffSource::WorkingTree,
        InputMode::Normal,
        Vec::new(),
        None,
        None,
    )
    .expect("build app");
    app.diff_state.viewport_width = 100;
    app.diff_state.viewport_height = 40;
    app.rebuild_annotations();
    app
}

fn spec_app() -> App {
    app_with(vec![file("spec.md", SPEC)], VcsType::File)
}

fn rendered_spec_app() -> App {
    let mut app = spec_app();
    app.set_markdown_rendered(true);
    app
}

/// Index of the rendered row whose text contains `needle`.
fn row_with(app: &App, needle: &str) -> usize {
    app.line_annotations
        .iter()
        .enumerate()
        .position(|(idx, ann)| {
            matches!(ann, AnnotatedLine::MarkdownRow { .. })
                && app
                    .content_for_side(idx, LineSide::New)
                    .is_some_and(|text| text.contains(needle))
        })
        .unwrap_or_else(|| panic!("no rendered row contains {needle:?}"))
}

/// Index of the source diff line `lineno`.
fn source_row(app: &App, lineno: u32) -> usize {
    app.line_annotations
        .iter()
        .position(|ann| {
            matches!(ann, AnnotatedLine::DiffLine { new_lineno: Some(n), .. } if *n == lineno)
        })
        .unwrap_or_else(|| panic!("no source row for line {lineno}"))
}

fn markdown_rows(app: &App) -> usize {
    app.line_annotations
        .iter()
        .filter(|ann| matches!(ann, AnnotatedLine::MarkdownRow { .. }))
        .count()
}

fn comment_on_cursor(app: &mut App, text: &str) {
    crate::handler::handle_diff_action(app, Action::AddLineComment);
    assert_eq!(app.input_mode, InputMode::Comment);
    app.comment_buffer = text.to_string();
    app.save_comment();
}

/// `(range, side)` of every stored line comment on `spec.md`.
fn stored_targets(app: &App) -> Vec<(LineRange, Option<LineSide>)> {
    let review = app
        .session
        .files
        .get(&PathBuf::from("spec.md"))
        .expect("spec registered");
    let mut out: Vec<_> = review
        .line_comments
        .iter()
        .flat_map(|(line, comments)| {
            comments
                .iter()
                .map(move |c| (c.line_range.unwrap_or(LineRange::single(*line)), c.side))
        })
        .collect();
    out.sort_by_key(|(range, _)| (range.start, range.end));
    out
}

fn draw(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| crate::ui::render(frame, app))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn screen_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn whole_markdown_files_from_file_sources_are_eligible() {
    let app = spec_app();
    assert!(app.markdown_eligible(&app.diff_files[0]));

    let git = app_with(vec![file("spec.md", SPEC)], VcsType::Git);
    assert!(
        !git.markdown_eligible(&git.diff_files[0]),
        "a Git diff keeps its source rows so deletions stay visible"
    );
    let code = app_with(vec![file("main.rs", &["fn main() {}"])], VcsType::File);
    assert!(!code.markdown_eligible(&code.diff_files[0]));
}

#[test]
fn rendered_rows_replace_source_rows_and_every_height_mirror_agrees() {
    let mut app = rendered_spec_app();
    assert!(markdown_rows(&app) > 0);
    assert!(
        !app.line_annotations
            .iter()
            .any(|ann| matches!(ann, AnnotatedLine::DiffLine { .. })),
        "rendered rows stand in for source rows"
    );
    assert_eq!(app.line_annotations.len(), app.total_lines());

    // A comment adds boxes after its block; the mirrors still agree.
    app.diff_state.cursor_line = row_with(&app, "ends on five");
    comment_on_cursor(&mut app, "tighten this");
    assert!(
        app.line_annotations
            .iter()
            .any(|ann| matches!(ann, AnnotatedLine::LineComment { .. }))
    );
    assert_eq!(app.line_annotations.len(), app.total_lines());
    for idx in 0..app.line_annotations.len() {
        if matches!(app.line_annotations[idx], AnnotatedLine::MarkdownRow { .. }) {
            assert_eq!(
                crate::ui::row_height::annotation_row_height(&app, idx),
                1,
                "pre-wrapped rows fit"
            );
        }
    }
}

#[test]
fn the_renderer_draws_each_rendered_row_on_its_annotation() {
    let mut app = rendered_spec_app();
    app.show_file_list = false;
    app.diff_state.cursor_line = row_with(&app, "A paragraph");
    comment_on_cursor(&mut app, "first note");
    for width in [60u16, 120] {
        let buffer = draw(&mut app, width, 40);
        assert_eq!(app.line_annotations.len(), app.total_lines());
        let rows = screen_rows(&buffer);
        for needle in [
            "Spec title",
            "ends on five",
            "row │ value",
            "Final bold words.",
        ] {
            let screen_row = rows
                .iter()
                .position(|r| r.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} not drawn at {width}: {rows:#?}"));
            let idx = app
                .diff_annotation_at_screen_row(screen_row as u16)
                .expect("drawn row maps to an annotation");
            let text = app.content_for_side(idx, LineSide::New).unwrap_or_default();
            assert!(
                text.contains(needle),
                "screen row {screen_row} shows {needle:?} but maps to {:?}",
                app.line_annotations[idx]
            );
        }
        assert!(
            rows.iter()
                .any(|r| r.contains("rendered Markdown · lines 1–13")),
            "{rows:#?}"
        );
        assert!(rows.iter().any(|r| r.contains("first note")));
        assert!(
            rows.iter()
                .any(|r| r.contains("line 11 hidden in rendered view")),
            "hidden source gets a marker: {rows:#?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("**") || r.contains("|---")),
            "markup is gone: {rows:#?}"
        );
    }
}

#[test]
fn the_same_block_gives_the_same_target_in_either_view_and_at_any_width() {
    // Rendered, wide.
    let mut app = rendered_spec_app();
    app.diff_state.cursor_line = row_with(&app, "A paragraph");
    comment_on_cursor(&mut app, "wide");
    // Rendered, narrow: the paragraph wraps differently, the target does not.
    app.sync_viewport_width(30);
    app.diff_state.cursor_line = row_with(&app, "five.");
    comment_on_cursor(&mut app, "narrow");
    // Source view: a visual selection over lines 3-5.
    app.set_markdown_rendered(false);
    app.diff_state.cursor_line = source_row(&app, 3);
    app.enter_visual_mode_at_cursor();
    app.diff_state.cursor_line = source_row(&app, 5);
    app.extend_visual_to_cursor();
    app.enter_comment_from_visual();
    app.comment_buffer = "source".into();
    app.save_comment();

    let expected = (LineRange::new(3, 5), Some(LineSide::New));
    assert_eq!(stored_targets(&app), vec![expected, expected, expected]);
}

#[test]
fn a_one_line_block_and_a_table_row_target_their_own_lines() {
    let mut app = rendered_spec_app();
    app.diff_state.cursor_line = row_with(&app, "Spec title");
    comment_on_cursor(&mut app, "title");
    app.diff_state.cursor_line = row_with(&app, "row │ value");
    comment_on_cursor(&mut app, "row");
    assert_eq!(
        stored_targets(&app),
        vec![
            (LineRange::single(1), Some(LineSide::New)),
            (LineRange::single(9), Some(LineSide::New)),
        ]
    );
}

#[test]
fn a_visual_selection_across_blocks_takes_the_smallest_covering_range() {
    let mut app = rendered_spec_app();
    app.diff_state.cursor_line = row_with(&app, "A paragraph");
    app.enter_visual_mode_at_cursor();
    app.diff_state.cursor_line = row_with(&app, "row │ value");
    app.extend_visual_to_cursor();
    assert_eq!(
        app.visual_selection_line_range(),
        Some((LineRange::new(3, 9), LineSide::New))
    );
}

#[test]
fn source_comments_render_once_after_the_block_holding_their_line() {
    let mut app = spec_app();
    // Line 4 sits inside the paragraph (3-5); line 6 is the blank after it.
    for line in [4, 6] {
        app.diff_state.cursor_line = source_row(&app, line);
        comment_on_cursor(&mut app, &format!("on {line}"));
    }
    app.set_markdown_rendered(true);
    let paragraph_end = row_with(&app, "five.");
    let comment_lines: Vec<u32> = app
        .line_annotations
        .iter()
        .filter_map(|ann| match ann {
            AnnotatedLine::LineComment { line, .. } => Some(*line),
            _ => None,
        })
        .collect();
    let rows_per_box = comment_lines.iter().filter(|&&l| l == 4).count();
    assert_eq!(
        comment_lines.len(),
        rows_per_box * 2,
        "each comment renders once: {comment_lines:?}"
    );
    assert!(
        matches!(
            app.line_annotations[paragraph_end + 1],
            AnnotatedLine::LineComment { line: 4, .. }
        ),
        "boxes follow the paragraph's last row"
    );
    assert!(
        matches!(
            app.line_annotations[paragraph_end + 1 + rows_per_box],
            AnnotatedLine::LineComment { line: 6, .. }
        ),
        "a blank-line key joins the block above, after lower keys"
    );
}

#[test]
fn toggling_the_view_keeps_the_cursor_on_its_source_line() {
    let mut app = spec_app();
    app.diff_state.cursor_line = source_row(&app, 9);
    app.set_markdown_rendered(true);
    assert_eq!(app.markdown_range_at_cursor(), Some(LineRange::single(9)));
    app.set_markdown_rendered(false);
    assert_eq!(app.get_line_at_cursor(), Some((9, LineSide::New)));
}

#[test]
fn resizing_rewraps_and_keeps_the_cursor_on_its_block() {
    let mut app = rendered_spec_app();
    app.diff_state.cursor_line = row_with(&app, "Final");
    let wide_rows = markdown_rows(&app);
    app.sync_viewport_width(24);
    assert!(markdown_rows(&app) > wide_rows, "narrower panes wrap more");
    assert_eq!(app.markdown_range_at_cursor(), Some(LineRange::single(13)));
    assert_eq!(app.line_annotations.len(), app.total_lines());
}

#[test]
fn jumping_to_a_source_line_lands_on_its_block() {
    let mut app = rendered_spec_app();
    app.go_to_source_line(4, LineSide::New);
    assert_eq!(app.markdown_range_at_cursor(), Some(LineRange::new(3, 5)));
    assert_eq!(
        app.diff_state.cursor_line,
        row_with(&app, "A paragraph"),
        "the block's first row"
    );
}

#[test]
fn search_matches_rendered_text() {
    let mut app = rendered_spec_app();
    app.search_buffer = "bold words".into();
    assert!(app.search_in_diff_from_cursor());
    assert_eq!(app.diff_state.cursor_line, row_with(&app, "Final"));
    app.search_buffer = r"\*\*".into();
    assert!(!app.search_in_diff_from_cursor(), "markup is not rendered");
}

#[test]
fn side_by_side_shows_markdown_as_source() {
    let mut app = rendered_spec_app();
    app.toggle_diff_view_mode();
    assert_eq!(app.diff_view_mode, DiffViewMode::SideBySide);
    assert_eq!(markdown_rows(&app), 0);
    assert_eq!(app.line_annotations.len(), app.total_lines());

    // Turning rendered view on from side-by-side switches to unified.
    let mut app = spec_app();
    app.toggle_diff_view_mode();
    app.set_markdown_rendered(true);
    assert_eq!(app.diff_view_mode, DiffViewMode::Unified);
    assert!(markdown_rows(&app) > 0);
}

#[test]
fn hunk_review_from_a_rendered_row_collapses_to_the_label() {
    let mut app = rendered_spec_app();
    app.diff_state.cursor_line = row_with(&app, "A paragraph");
    app.toggle_hunk_reviewed();
    assert!(app.is_hunk_reviewed(0, 0));
    assert_eq!(markdown_rows(&app), 0);
    assert_eq!(app.line_annotations.len(), app.total_lines());
}

#[test]
fn a_changed_reload_warns_about_comment_anchors_but_a_toggle_does_not() {
    let mut app = rendered_spec_app();
    app.diff_state.cursor_line = row_with(&app, "A paragraph");
    comment_on_cursor(&mut app, "note");
    app.set_markdown_rendered(false);
    app.set_markdown_rendered(true);
    app.sync_viewport_width(50);
    let warned = |app: &App| {
        app.message
            .as_ref()
            .is_some_and(|m| m.content.contains("anchored to an earlier version"))
    };
    assert!(!warned(&app), "toggle and resize alone never warn");

    let mut edited = SPEC.to_vec();
    edited.insert(0, "Inserted line");
    app.diff_files = vec![file("spec.md", &edited)];
    app.rebuild_annotations();
    assert!(warned(&app));
}

#[test]
fn copy_text_and_editor_line_come_from_the_rendered_row() {
    let app = {
        let mut app = rendered_spec_app();
        app.diff_state.cursor_line = row_with(&app, "Final");
        app
    };
    assert_eq!(
        app.content_for_side(app.diff_state.cursor_line, LineSide::New),
        Some("Final bold words.")
    );
    assert_eq!(app.get_line_at_cursor(), Some((13, LineSide::New)));
}

/// Points session storage at a fresh temp dir for this test thread, and puts
/// it back on drop, so a failing assertion cannot leak the override into the
/// next test the worker runs. Mirrors `TestReviewsDirGuard` in
/// `persistence/storage.rs`.
struct ReviewsDirGuard {
    dir: tempfile::TempDir,
}

impl ReviewsDirGuard {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::persistence::storage::set_test_reviews_dir(Some(dir.path().to_path_buf()));
        Self { dir }
    }

    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

impl Drop for ReviewsDirGuard {
    fn drop(&mut self) {
        // Runs before `dir` is removed.
        crate::persistence::storage::set_test_reviews_dir(None);
    }
}

#[test]
fn the_reviews_dir_override_is_cleared_even_when_a_test_panics() {
    let mut overridden = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let reviews = ReviewsDirGuard::new();
        overridden = Some(reviews.path().to_path_buf());
        panic!("a failing assertion");
    }));
    assert!(result.is_err());
    let after = crate::persistence::storage::get_reviews_dir().expect("reviews dir");
    assert_ne!(
        Some(after),
        overridden,
        "the override did not outlive the panic"
    );
}

/// The durable loop the feature exists for: a comment made in rendered view is
/// saved to disk, reopens in a fresh app with the same source range in both
/// views, and `tuicr review comments` reports it in the existing JSON shape.
#[test]
fn a_rendered_comment_persists_reopens_and_exports_unchanged() {
    let reviews = ReviewsDirGuard::new();

    // Comment in rendered view; `save_comment` autosaves the session.
    let mut app = rendered_spec_app();
    app.diff_state.cursor_line = row_with(&app, "A paragraph");
    comment_on_cursor(&mut app, "Say which errors are retryable.");
    assert!(
        app.message
            .as_ref()
            .is_some_and(|m| m.content == "Comment added to lines 3-5"),
        "{:?}",
        app.message
    );
    drop(app);

    // Reopen from disk.
    let store = crate::review_store::ReviewStore::with_reviews_dir(reviews.path());
    let sessions = store.list_all_sessions().expect("list sessions");
    assert_eq!(sessions.len(), 1, "one persisted session");
    assert_eq!(sessions[0].comment_count, 1);
    let session_path = sessions[0].session_ref.path().to_path_buf();
    let loaded = store
        .get_review(&sessions[0].session_ref)
        .expect("load session");

    let expected = vec![(LineRange::new(3, 5), Some(LineSide::New))];
    let mut reopened = app_with_session(vec![file("spec.md", SPEC)], VcsType::File, loaded);
    assert_eq!(stored_targets(&reopened), expected);

    // Source view: the box follows line 5, the range's end.
    let after_line_5 = source_row(&reopened, 5) + 1;
    assert!(matches!(
        reopened.line_annotations[after_line_5],
        AnnotatedLine::LineComment { line: 5, .. }
    ));
    // Rendered view: the box follows the paragraph's last row.
    reopened.set_markdown_rendered(true);
    let after_paragraph = row_with(&reopened, "five.") + 1;
    assert!(matches!(
        reopened.line_annotations[after_paragraph],
        AnnotatedLine::LineComment { line: 5, .. }
    ));
    assert_eq!(stored_targets(&reopened), expected);

    // `tuicr review comments --session <path>`: the existing JSON shape.
    let mut out = Vec::new();
    crate::review_cli::run_with_writer(
        crate::cli::ReviewCommand::Comments {
            session: session_path.display().to_string(),
            repo: PathBuf::from("."),
        },
        &mut out,
    )
    .expect("review comments");
    let json: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    let comments = json.as_array().expect("a JSON array");
    assert_eq!(comments.len(), 1);
    let comment = comments[0].as_object().expect("an object");
    let mut keys: Vec<&str> = comment.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "author",
            "comment_type",
            "content",
            "created_at",
            "end_line",
            "id",
            "lifecycle_state",
            "location",
            "path",
            "side",
            "start_line",
        ],
        "no rendered-view fields leak into the export"
    );
    assert_eq!(comment["location"], "spec.md:3-5");
    assert_eq!(comment["path"], "spec.md");
    assert_eq!(comment["start_line"], 3);
    assert_eq!(comment["end_line"], 5);
    assert_eq!(comment["side"], "new");
    assert_eq!(comment["lifecycle_state"], "local_draft");
    assert_eq!(comment["content"], "Say which errors are retryable.");
}
