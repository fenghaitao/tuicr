# Rendered Markdown review: Conductor/AutoDML integration

Status: Proposed  
Date: 2026-10-19  
Audience: Conductor/AutoDML maintainers

This note covers how the Conductor/AutoDML workflow would consume tuicr's rendered Markdown review. The tuicr feature itself is specified in [tuicr-rendered-markdown-review.md](tuicr-rendered-markdown-review.md). It ships independently and does not depend on this integration. This note probably belongs in the conductor-autodml repository; it sits here only until it moves.

## Workflow context

Conductor/AutoDML work requires human review of both specifications and implementation. Specifications may need approval before implementation, including sections that have no Git changes. Implementation review happens at task and iteration boundaries, and findings need to survive interruptions and be consumable by the workflow.

The AutoDML workflow records an iteration's starting commit as `base_0`, packages the iteration diff, writes `final-review.md`, and routes findings into a fix and re-review cycle. A terminal application's latest agent turn is not an adequate substitute for those task and iteration boundaries.

Evidence inspected, in the conductor-autodml repository: `README.md`, `phases/shared/implement.yaml` (`base_0`, `final_package`, `final_review`, and fix/re-review routing), and `lib/implement/review-package.py`.

## Review scope

For AutoDML, review a complete spec and the relevant implementation scope in separate panes or sessions:

```bash
tuicr --file specs/design.md          # the spec, as a complete document
tuicr -r BASE_0..HEAD                 # the iteration's implementation
```

The workflow selects the recorded task/iteration baseline and reviewed paths. Do not infer review scope from pane activity or replace the existing review package with an arbitrary branch diff.

The repository's `skills/tuicr/` agent skill already launches tuicr in a cmux, tmux, Zellij, or Herdr split pane; an adapter can build on it.

## Proposed adapter workflow

1. At the existing specification review boundary, open the complete spec with tuicr.
2. Let the user write comments in rendered or source view.
3. Discover and record the exact session ID/path through `tuicr review list`.
4. Read comments using `tuicr review comments --session SESSION`.
5. Save exported feedback alongside the workflow's artifacts, together with target identity and source snapshot information.
6. Supply spec feedback to the spec revision step; supply implementation feedback to the appropriate fix step.

```bash
tuicr review comments --session SESSION > human-spec-review.json
```

The adapter must record the repository or standalone document path, reviewed commit where applicable, content digest for a mutable spec, review scope, and completion decision. Those are workflow metadata, not tuicr comment fields.

## Workflow policy

- Pause automated edits to a spec while a human reviews that snapshot. Tuicr warns when a reload brings changed content under existing comments, but it does not reanchor them.
- Do not infer approval from zero comments or a CLI exit code. Review progress is useful evidence, but the existing Conductor human decision remains the acceptance boundary.
- Do not make a source-code TUI the authority for DML/Simics correctness: existing contract checks, builds, tests, and automated review remain part of AutoDML's validation.
- The deployed AutoDML setup contains a VP repository and a nested workflow repository. Resolve each review against the actual owning repository or explicit spec path; do not assume all specs and production files share one Git root.
- Existing tuicr session identity and file-mode persistence need regression coverage before promising long-lived AutoDML review artifacts.
