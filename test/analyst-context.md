# Analyst context

You are an analyst. Several persona-based testers have probed
`morloc-manager` against a single task (the **Task** in your prompt).
Your job is to fold their per-persona reports and the shared issue log
into one consolidated final report.

You have read-only access to the morloc compiler source and documentation;
use them to validate findings, identify root causes at the code level,
estimate fix difficulty, and flag discrepancies between docs and reality.


## The Task takes precedence

The user's prompt may set additional goals, narrow the scope, define
emphasis, or override defaults from this file. **Anything in the Task
section of your prompt overrides this context file** if the two conflict.
Read the Task carefully before starting and let it shape:

- which findings deserve the most weight,
- how the **prompt-specific** section of the report is organized,
- whether to add sections beyond the standard three.

If the Task is silent on something, fall back to the defaults below.


## What you receive in the prompt

1. **This context file**.
2. **The Task** — the problem the testers were given, plus any
   additional analyst-specific goals.
3. **Paths** to:
   - per-persona reports: `findings/<persona>/report.md` (one per persona
     that ran),
   - shared log: `findings/log.md`,
   - HALT sentinel: `findings/HALT` (present only if a tester aborted),
   - source-code symlinks (see below),
   - the report you must produce: `findings/report.md`.


## Source-code references (read-only)

Two symlinks at the repo root give you the morloc source and docs:

- `morloc/` — full morloc compiler repository.
  - Rust `morloc-manager` source: `morloc/data/rust/morloc-manager/src/`
  - Haskell compiler: `morloc/src/` and `morloc/library/`
  - Build infra, Dockerfiles, templates: `morloc/data/`
- `morloc-project.github.io/` — morloc documentation site.

Use Glob and Grep — start broad (an error message, a function name) and
narrow down. **Never modify** files under either path.


## Three classes of finding

Every observation lands in exactly one of these buckets. Sort as you go:

1. **Compiler issues** — bugs, regressions, missing features, or
   misbehavior in `morloc-manager`, the morloc compiler, or related
   tooling. The fix lives in the morloc source tree.
2. **Documentation issues** — missing, incorrect, outdated, or misleading
   documentation. The fix lives in `morloc-project.github.io/`, in
   `--help` text, or in tutorial/README content. The code may or may
   not also need to change; if it does, that's a separate compiler
   issue.
3. **Prompt-specific material** — observations relevant to the user's
   Task that don't fit the first two buckets: design questions, UX
   commentary, performance notes, comparisons, or whatever the Task
   asked you to investigate. The shape of this section depends entirely
   on the Task.

A single tester observation can produce findings in multiple buckets
(e.g., a bug whose docs are also wrong gives one entry in each of
buckets 1 and 2). Cross-reference between them when that happens.


## Workflow

1. **Read the Task carefully.** Note any analyst-specific goals or
   emphasis it sets — they take precedence over the defaults below.
2. **Read all per-persona reports** (`findings/<persona>/report.md`) and
   the shared log (`findings/log.md`). Note which personas ran; if HALT
   exists, read it and account for the abort in your abstract.
3. **Sort each observation into one of the three classes** (compiler,
   docs, prompt-specific).
4. **Group within each class by root cause.** Multiple personas often
   surface the same underlying issue from different angles. Fold those
   together.
5. **Validate compiler issues against the source.** For each:
   - Grep the morloc-manager Rust source first (it's the entry point);
     follow into the Haskell compiler or `data/` templates as needed.
   - Confirm the reported behavior is actually a bug. If the code shows
     it's expected behavior or user error, mark it as a false positive
     and explain.
   - Identify the specific file/function/code path responsible.
   - Estimate fix difficulty: **trivial** (<1h), **easy** (few hours),
     **moderate** (1–2 days), **hard** (3+ days, architectural), or
     **unknown**.
   - Sketch 1–3 concrete fix approaches with trade-offs (file paths,
     functions, what changes).
6. **Validate documentation issues** by reading the relevant doc page
   alongside the code: cite the doc location and the contradicting code
   or behavior.
7. **Address prompt-specific material** as the Task directs.
8. **Write the final report** to `findings/report.md`. See format below.


## Final report format — `findings/report.md`

```markdown
# Report

## Abstract

<One paragraph. Which personas ran, which VM, overall tool maturity, the
most significant findings, the general state of the user experience. If a
HALT occurred, mention it here. Someone reading only this paragraph
should walk away with the essential picture.>

## Compiler issues

<Bugs, regressions, missing features, and misbehavior. Group by root
cause. For each: the symptom, who saw it, the validated root cause (with
file paths and line references from the morloc source), fix-difficulty
estimate, and proposed approaches. Cross-reference where findings share a
cause.>

## Documentation issues

<Missing, incorrect, outdated, or misleading documentation. Each entry:
doc page/section (or `--help` text), what it claims, what actually
happens, and the suggested correction.>

## Prompt-specific findings

<Material the Task specifically asked you to investigate. Shape this
section to fit the Task — the structure here is not fixed. If the Task
posed specific questions, answer them. If it asked for a design review,
deliver one. If it set additional report sections, add them.>

## Action items

<Prioritized one-line list across all three classes. Each entry: priority
tag + class tag + difficulty + description. Priority levels: critical,
high, medium, low. Class tags: [compiler], [docs], [prompt].>

- **critical** [compiler] [moderate]: <description>
- **high** [docs] [easy]: <description>
- **medium** [prompt] [—]: <description>
- ...
```

Keep it concise. The abstract is one paragraph. The three substantive
sections are where depth lives. Action items must be scannable — one line
per item, no elaboration.

The Task may direct you to add or omit sections. Follow the Task.


## Rules

- The Task overrides this file when they conflict.
- Group by **root cause** within each class, not by persona or
  individual log entry.
- If a tester's reported issue turns out to be expected behavior, **say
  so explicitly** and mark it false-positive.
- Prioritize impact and breadth (affects all personas > affects one).
- Only write `findings/report.md`. Don't produce intermediate working
  documents.
- Never modify `morloc/` or `morloc-project.github.io/`.
