# Diagnostic Loop Log

## Purpose
Track agent failures, attribute them to harness layers, fix the layer, and rerun.
This is the core method of harness engineering: failures are signals of structural defects.

## How to Use
1. Pick a task that tends to fail repeatedly.
2. Run the agent once.
3. Record the actual failure (not just "it didn't work").
4. Attribute it to exactly one layer: task, context, environment, verification, or state.
5. Fix only that layer.
6. Rerun the same task and compare results.
7. Repeat 3–5 rounds on the same task.

## Definitions
- **Task specification**: request was vague or underspecified.
- **Context provision**: conventions, architecture, or business rules were missing.
- **Execution environment**: setup, dependencies, versions, or tools were broken.
- **Verification feedback**: no tests, lint, or type checks available or enforced.
- **State management**: next session lost discoveries and started from scratch.

---

## Task Definition

### Task
<!-- Paste task description here -->

### Definition of Done
<!-- Write explicit, command-verified completion criteria -->
- [ ] New endpoint / feature exists
- [ ] Matches project conventions
- [ ] Tests pass: `<pytest command>`
- [ ] Lint passes: `<lint command>`
- [ ] Type checks pass: `<mypy/tsc command>`

---

## Run 1

| Field | Value |
|---|---|
| **Result** | Failed / Passed |
| **Observed failure** | Describe exactly what broke (error message, wrong behavior, missing output) |
| **Layer** | task / context / environment / verification / state |
| **Root cause** | What was missing or wrong in that layer? |
| **Fix applied** | What changed in AGENTS.md, init.sh, tests, or progress file? |

---

## Run 2

| Field | Value |
|---|---|
| **Result** | Failed / Passed |
| **Observed failure** | |
| **Layer** | task / context / environment / verification / state |
| **Root cause** | |
| **Fix applied** | |

---

## Run 3

| Field | Value |
|---|---|
| **Result** | Failed / Passed |
| **Observed failure** | |
| **Layer** | task / context / environment / verification / state |
| **Root cause** | |
| **Fix applied** | |

---

## Run 4

| Field | Value |
|---|---|
| **Result** | Failed / Passed |
| **Observed failure** | |
| **Layer** | task / context / environment / verification / state |
| **Root cause** | |
| **Fix applied** | |

---

## Run 5

| Field | Value |
|---|---|
| **Result** | Failed / Passed |
| **Observed failure** | |
| **Layer** | task / context / environment / verification / state |
| **Root cause** | |
| **Fix applied** | |

---

## Summary

| Layer | # Failures |
|---|---|
| task | |
| context | |
| environment | |
| verification | |
| state | |

Final result: Task succeeded after _____ rounds.
Main bottleneck: <layer name>