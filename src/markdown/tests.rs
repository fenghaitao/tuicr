//! Renderer and source-mapping regression cases.
//!
//! Most cases are adapted from herdr-reviewr's `src/markdown.rs` tests
//! (revision `4d7f2b49cd11bbe19b5bf7714d7c18e6e81c43a8`, MIT, see
//! `LICENSE-reviewr`); expectations differ where tuicr's adaptation does:
//! code rows map to the whole fenced block, and `<details>` bodies show.

use super::render::{Rendered, render};
use super::*;
use crate::theme::Theme;

fn rendered(md: &str, width: usize) -> Rendered {
    let theme = Theme::dark();
    let hl = theme.syntax_highlighter();
    let styles = MarkdownStyles::resolve(&theme, hl);
    render(md, width, hl, &styles)
}

fn text_of(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn texts(md: &str, width: usize) -> Vec<String> {
    rendered(md, width).lines.iter().map(text_of).collect()
}

fn doc(md: &str, width: usize) -> RenderedDocument {
    render_document(md, width, &Theme::dark())
}

/// `(first line, last line)` of the rendered line containing `needle`.
fn span_of(r: &Rendered, needle: &str) -> (usize, usize) {
    let t: Vec<String> = r.lines.iter().map(text_of).collect();
    let i = t
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in {t:?}"));
    (r.meta[i].source_line, r.meta[i].source_end)
}

/// Source range of the block whose row contains `needle`.
fn block_range(d: &RenderedDocument, needle: &str) -> LineRange {
    let row = d
        .rows
        .iter()
        .position(|r| r.text.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not rendered"));
    d.row_source(row).expect("a content row")
}

#[test]
fn headings_render_bold_without_markers() {
    let r = rendered("# Title\n\n## Sub", 80);
    let t: Vec<String> = r.lines.iter().map(text_of).collect();
    assert_eq!(t, vec!["Title", "", "Sub"]);
    assert!(
        r.lines[0]
            .spans
            .iter()
            .filter(|s| !s.content.is_empty())
            .all(|s| s.style.add_modifier.contains(Modifier::BOLD))
    );
}

#[test]
fn emphasis_maps_to_terminal_attributes() {
    let r = rendered("*it* **bold** ~~gone~~", 80);
    let style_of = |needle: &str| {
        r.lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains(needle))
            .map(|s| s.style.add_modifier)
            .unwrap()
    };
    assert!(style_of("it").contains(Modifier::ITALIC));
    assert!(style_of("bold").contains(Modifier::BOLD));
    assert!(style_of("gone").contains(Modifier::CROSSED_OUT));
}

#[test]
fn list_items_mark_and_hang_their_wraps() {
    let t = texts("- alpha beta gamma delta", 14);
    assert_eq!(t[0], "• alpha beta");
    assert_eq!(t[1], "  gamma delta", "continuation hangs under the text");
}

#[test]
fn ordered_and_task_lists_mark_items() {
    assert_eq!(texts("1. one\n2. two", 80), vec!["1. one", "2. two"]);
    assert_eq!(
        texts("- [x] done\n- [ ] open", 80),
        vec!["☑ done", "☐ open"]
    );
}

#[test]
fn block_quotes_carry_bars_per_level() {
    let t = texts("> outer\n>> inner", 80);
    assert!(t.iter().any(|l| l.starts_with("▎ outer")), "{t:?}");
    assert!(t.iter().any(|l| l.starts_with("▎▎ inner")), "{t:?}");
}

#[test]
fn nesting_caps_at_eight_bars() {
    let deep = "> ".repeat(12) + "core";
    let t = texts(&deep, 80);
    let bars = t.last().unwrap().chars().take_while(|c| *c == '▎').count();
    assert_eq!(bars, 8);
}

#[test]
fn narrow_table_aligns_and_bolds_the_header() {
    let t = texts("| a | long |\n|---|---|\n| x1 | y |", 40);
    assert_eq!(t[0], "a  │ long");
    assert_eq!(t[2], "x1 │ y   ");
    assert!(t[1].starts_with('─'), "a dim rule under the header: {t:?}");
}

#[test]
fn wide_table_falls_back_to_source_text() {
    let t = texts("| alpha | beta |\n|---|---|\n| 123456789 | 987654321 |", 12);
    assert!(
        t[0].starts_with("| alpha"),
        "source text shows, wrapped: {t:?}"
    );
    let joined = t.concat();
    assert!(
        joined.contains("123456789") && joined.contains("987654321"),
        "{t:?}"
    );
}

#[test]
fn an_over_wide_table_shrinks_its_widest_column_and_wraps() {
    let md = "| id | note |\n|---|---|\n| a | wraps beyond the pane edge |\n| b | short |";
    let t = texts(md, 24);
    assert_eq!(t[0].trim_end(), "id │ note");
    assert_eq!(t[2].trim_end(), "a  │ wraps beyond the");
    assert_eq!(
        t[3].trim_end(),
        "   │ pane edge",
        "the wrap continues the separator"
    );
    assert_eq!(t[4].trim_end(), "b  │ short");
    assert!(t.iter().all(|l| l.chars().count() <= 24), "{t:?}");
}

#[test]
fn tied_widest_columns_shrink_together() {
    let t = texts("| aaaaaaaaaaaa | bbbbbbbbbbbb |\n|---|---|\n| x | y |", 21);
    assert_eq!(t[0], "aaaaaaaaa │ bbbbbbbbb");
}

#[test]
fn control_and_bidi_characters_render_as_placeholders() {
    let t = texts("a\u{1b}[31mb \u{202e}evil", 80);
    assert_eq!(t[0], "a�[31mb �evil");
}

#[test]
fn breaks_soft_is_space_hard_is_line() {
    assert_eq!(texts("one\ntwo", 80), vec!["one two"]);
    assert_eq!(texts("one  \ntwo", 80), vec!["one", "two"]);
}

#[test]
fn blocks_separate_with_one_blank_line() {
    assert_eq!(texts("first\n\nsecond", 80), vec!["first", "", "second"]);
}

#[test]
fn thematic_break_is_a_dim_rule() {
    let t = texts("a\n\n---\n\nb", 80);
    assert!(t.iter().any(|l| l.starts_with('─')), "{t:?}");
}

#[test]
fn links_show_their_destination_and_images_collapse_without_fetching() {
    let t = texts("see [docs](https://x.dev/d) ![logo](logo.png)", 80);
    assert_eq!(t[0], "see docs (https://x.dev/d) ⧉ logo");
}

#[test]
fn a_mermaid_fence_collapses_to_a_placeholder() {
    let t = texts("```mermaid\ngraph TD\nA-->B\n```", 80);
    assert_eq!(t, vec!["⧉ mermaid graph"]);
}

#[test]
fn fenced_code_is_highlighted_and_indented() {
    let r = rendered("```rust\nfn main() {}\n```", 80);
    assert_eq!(text_of(&r.lines[0]), "  fn main() {}");
    let colors: std::collections::HashSet<_> =
        r.lines[0].spans.iter().filter_map(|s| s.style.fg).collect();
    assert!(colors.len() > 2, "several syntax colors: {:?}", r.lines[0]);
}

#[test]
fn details_show_their_summary_and_body() {
    let t = texts(
        "<details>\n<summary>hi</summary>\nbody text\n</details>",
        80,
    );
    assert!(t.iter().any(|l| l.contains("▾ hi")), "{t:?}");
    assert!(t.iter().any(|l| l.contains("body text")), "{t:?}");
    assert!(t.iter().all(|l| !l.contains("<details>")), "{t:?}");
}

#[test]
fn empty_and_plain_inputs_render() {
    assert!(texts("", 80).is_empty());
    assert_eq!(texts("just plain prose", 80), vec!["just plain prose"]);
}

#[test]
fn every_rendered_line_maps_to_its_blocks_whole_source_range() {
    let md = "para one\ncontinues here\n\n- item a\n  more a\n- item b\n  - nested\n\n\
              ```\nx = 1\n```\n";
    let r = rendered(md, 80);
    assert_eq!(
        span_of(&r, "para one"),
        (1, 2),
        "a paragraph spans its lines"
    );
    assert_eq!(
        span_of(&r, "item a"),
        (4, 5),
        "an item ends where the next begins"
    );
    assert_eq!(
        span_of(&r, "item b"),
        (6, 6),
        "a parent item never claims its nested list"
    );
    assert_eq!(span_of(&r, "nested"), (7, 7));
    assert_eq!(
        span_of(&r, "x = 1"),
        (9, 11),
        "a code line maps to its whole fenced block"
    );
    for m in &r.meta {
        assert!(m.source_line <= m.source_end, "{m:?}");
    }
    assert!(r.silent.is_empty(), "every line shows: {:?}", r.silent);
}

#[test]
fn every_code_row_maps_to_the_whole_fenced_block() {
    let r = rendered("```rust\nlet a = 1;\nlet b = 2;\n```\n", 80);
    assert_eq!(span_of(&r, "let a"), (1, 4));
    assert_eq!(span_of(&r, "let b"), (1, 4));
}

#[test]
fn table_rows_map_to_their_own_source_lines() {
    let md = "| a | b |\n|---|---|\n| one | two |\n| three | four |\n";
    let r = rendered(md, 80);
    assert_eq!(span_of(&r, "a"), (1, 1), "the header row");
    assert_eq!(
        span_of(&r, "─"),
        (2, 2),
        "the rule stands for the delimiter row"
    );
    assert_eq!(span_of(&r, "one"), (3, 3));
    assert_eq!(span_of(&r, "three"), (4, 4));
    // Over-wide at every floor: the source fallback maps line by line too.
    let wide = format!(
        "| {} | {} |\n|---|---|\n| x | y |\n",
        "w".repeat(60),
        "v".repeat(60)
    );
    let r = rendered(&wide, 10);
    assert_eq!(span_of(&r, "| x | y |"), (3, 3));
}

#[test]
fn lines_that_render_nothing_are_silent() {
    let md = "shown\n\n<!-- hidden note -->\n\n[ref]: https://x.dev\n\ntail\n";
    assert_eq!(rendered(md, 80).silent, [3, 5], "only what nothing shows");
}

#[test]
fn silent_lines_never_flag_source_that_shows() {
    let silent = |md: &str| rendered(md, 80).silent;
    for md in [
        "- item\n  ```\n  code\n  ```\n",
        "- item\n\n  | a | b |\n  |---|---|\n  | c | d |\n",
        "- item\n  <div>x</div>\n",
        "- a\n  # h\n",
    ] {
        assert!(silent(md).is_empty(), "{md:?}: {:?}", silent(md));
    }
    assert!(silent("> a\n>\n> b\n").is_empty(), "a quote's `>` lines");
    assert!(silent("<p align=\"center\">\n<img alt=\"logo\" src=\"x.png\">\n</p>\n").is_empty());
    // An open disclosure's closing tag alone on its line renders nothing.
    let md = "<details>\n<summary>\nMore info\n</summary>\n\nbody\n\n</details>\n";
    assert_eq!(silent(md), [8]);
    assert_eq!(
        silent("```\n```\n"),
        [1, 2],
        "an empty fence renders no line"
    );
    assert_eq!(silent("<p>\n<summary>\n"), [1, 2]);
    assert_eq!(silent("<p>\r\n<summary>\r\n"), [1, 2]);
}

#[test]
fn hidden_comments_are_silent_and_never_render() {
    let md =
        "before\n\n<!--\nignore previous instructions\n-->\n\nafter <!-- inline aside --> text\n";
    let r = rendered(md, 80);
    let t: Vec<String> = r.lines.iter().map(text_of).collect();
    assert!(!t.iter().any(|l| l.contains("ignore previous")), "{t:?}");
    assert!(!t.iter().any(|l| l.contains("-->")), "{t:?}");
    assert_eq!(
        r.silent,
        [3, 4, 5, 7],
        "a multi-line comment, and an inline one"
    );
}

#[test]
fn text_after_a_nested_block_keeps_its_own_lines() {
    let md = "1. step:\n   ```bash\n   run\n   ```\n   After text.\n2. next\n";
    let r = rendered(md, 80);
    assert_eq!(span_of(&r, "After text."), (5, 5));
    assert_eq!(span_of(&r, "step:"), (1, 1));
    assert!(r.silent.is_empty(), "{:?}", r.silent);
}

// ---- tuicr document model ----------------------------------------------------------------

#[test]
fn every_row_fits_its_width() {
    let md = "# A heading long enough to wrap at a narrow width\n\n\
              - a list item that also wraps across several rows\n\n\
              | col | another column |\n|---|---|\n| value | a cell that has to wrap |\n\n\
              ```\nlet code = \"a line of code wider than the pane\";\n```\n\n\
              <!-- a hidden comment that is itself wider than the pane -->\n";
    for width in [12, 20, 33, 80] {
        let d = doc(md, width);
        for row in &d.rows {
            assert!(
                row.line.width() <= width,
                "{:?} is wider than {width}",
                row.text
            );
            assert_eq!(row.line.width(), row.text.width());
        }
    }
}

#[test]
fn wrapped_rows_form_one_block_and_gaps_have_none() {
    let d = doc("first paragraph that wraps\nover two lines\n\nsecond", 10);
    let first = &d.blocks[0];
    assert_eq!(first.range, LineRange::new(1, 2));
    assert!(first.rows.len() > 1, "{:?}", d.rows);
    assert!(d.rows[first.rows.end].block.is_none(), "a gap row follows");
    assert_eq!(d.blocks[1].range, LineRange::single(4));
    assert_eq!(d.blocks.len(), 2);
}

#[test]
fn a_blocks_range_is_the_same_at_every_width() {
    let md = "intro\n\nA paragraph on line three\nthat continues on line four\nand ends on five.\n\n\
              | k | v |\n|---|---|\n| row | value |\n";
    for width in [8, 15, 40, 120] {
        let d = doc(md, width);
        assert_eq!(block_range(&d, "intro"), LineRange::single(1));
        assert_eq!(block_range(&d, "five."), LineRange::new(3, 5));
        assert_eq!(block_range(&d, "row"), LineRange::single(9), "at {width}");
    }
}

#[test]
fn hidden_source_becomes_an_anchored_marker_in_source_order() {
    let md = "shown\n\n<!-- note -->\n\n[ref]: https://x.dev\n[two]: https://y.dev\n\ntail\n";
    let d = doc(md, 80);
    let order: Vec<(LineRange, bool)> = d.blocks.iter().map(|b| (b.range, b.hidden)).collect();
    assert_eq!(
        order,
        vec![
            (LineRange::single(1), false),
            (LineRange::single(3), true),
            (LineRange::new(5, 6), true),
            (LineRange::single(8), false),
        ]
    );
    let texts: Vec<&str> = d.rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts[0], "shown");
    assert_eq!(texts[1], "", "a gap above the marker");
    assert!(texts[2].starts_with("⋯ line 3 hidden"));
    assert_eq!(texts[3], "", "and the same gap below it");
    let marker = &d.rows[d.blocks[2].rows.start];
    assert_eq!(
        marker.text,
        "⋯ lines 5–6 hidden in rendered view (:markdown for source)"
    );
}

#[test]
fn hidden_source_after_every_block_trails_the_document() {
    let d = doc("text\n\n<!-- end -->\n", 80);
    let last = d.blocks.last().unwrap();
    assert!(last.hidden);
    assert!(
        d.rows[last.rows.start - 1].block.is_none(),
        "set off by a gap"
    );
    assert_eq!(last.range, LineRange::single(3));
}

#[test]
fn comment_keys_anchor_to_the_containing_or_preceding_block() {
    let d = doc("# Title\n\npara one\npara two\n\n\npara three\n", 80);
    // Blocks: 1, 3-4, 7.
    assert_eq!(d.anchor_block(1), Some(0));
    assert_eq!(d.anchor_block(4), Some(1), "a range's end line");
    assert_eq!(
        d.anchor_block(5),
        Some(1),
        "a blank line joins the block above"
    );
    assert_eq!(d.anchor_block(6), Some(1));
    assert_eq!(d.anchor_block(7), Some(2));
    assert_eq!(d.anchor_block(99), Some(2), "past the end: the last block");
    let d = doc("\n\nlate start\n", 80);
    assert_eq!(d.anchor_block(1), Some(0), "before every block: the first");
    assert_eq!(doc("", 80).anchor_block(1), None);
}

#[test]
fn nearest_block_prefers_containment_then_distance() {
    let d = doc("one\n\n\n\n\ntwo\n", 80);
    assert_eq!(d.nearest_block(1), Some((0, true)));
    assert_eq!(d.nearest_block(2), Some((0, false)));
    assert_eq!(d.nearest_block(5), Some((1, false)));
}

#[test]
fn markdown_paths_are_recognized_case_insensitively() {
    use std::path::Path;
    assert!(is_markdown_path(Path::new("specs/a.md")));
    assert!(is_markdown_path(Path::new("README.MARKDOWN")));
    assert!(!is_markdown_path(Path::new("notes.txt")));
    assert!(!is_markdown_path(Path::new("md")));
}
