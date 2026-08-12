# OpenRoutine

The open-source, local-first alternative to Claude Code Routines and ChatGPT scheduled tasks: one machine-global daemon runs markdown-defined tasks through any AI agent CLI, and is at once the scheduler, the API, and the UI.

## Language

**Task**:
A unit of agent work, defined entirely by a single `.cron.md` file — frontmatter for metadata, body as the prompt. Its canonical identity is `<Project>/<filename stem>`; renaming or moving the file creates a new Task.
_Avoid_: routine, job, cron job

**Agent**:
A configured command template that accepts a prompt. Any CLI qualifies; OpenRoutine never talks to a model API itself.
_Avoid_: model, LLM, backend

**Tick**:
An instant at which a Task's schedule comes due. Every Tick becomes exactly one Run or one Skip.
_Avoid_: occurrence, trigger time

**Run**:
A single execution of a Task's prompt by its Agent, produced by a scheduled Tick or by a Fire.
_Avoid_: execution, invocation

**Skip**:
A Tick that was not run, recorded with its reason: the previous Run was still active (`overlap`), the Daemon was down when the Tick passed (`daemon-down`), or the Daemon was running but reached the Tick too late (`missed`). Never silent — every Tick becomes exactly one Run or one Skip.
_Avoid_: dropped run, lost tick

**Timeout**:
The wall-clock budget for a single Run. When it expires the Run's whole process group is ended and the Run is recorded as timed out — never left hanging.
_Avoid_: deadline, TTL

**Jitter**:
The deterministic offset between a Task's Tick and the moment it actually fires, derived from the Task id so it never changes between runs. Spreads a machine full of midnight Tasks without making any of them unpredictable.
_Avoid_: splay, randomization, fuzz

**Fire**:
Starting a Run on demand — via the API or UI — outside the schedule.
_Avoid_: trigger, manual invocation

**Dry run**:
Rendering a Task's fully resolved execution plan — command line, environment, working directory, upcoming fire times — without spawning anything. A read, not a Run; nothing is recorded.
_Avoid_: exec, preview, test run

**Manual**:
A Task with no schedule — neither `cron` nor `at` — that runs only when Fired.
_Avoid_: on-demand task, unscheduled task

**One-shot**:
A Task scheduled by a single future timestamp (`at`) instead of a recurrence. It fires once, then becomes Completed. Not the same as Manual.
_Avoid_: one-off, one-time task

**Completed**:
The state of a One-shot Task after its single fire (or after its time passed unrun). Machine state only — the file is untouched, and editing the timestamp re-arms the Task.
_Avoid_: done, finished, expired

**Disabled**:
A Task switched off in its own definition (frontmatter). Part of the reviewable, diffable file.
_Avoid_: paused

**Paused**:
A Task temporarily switched off at runtime. Machine-owned state, never part of the definition. A Task runs only when neither Disabled nor Paused. Pausing can also be daemon-wide: a persisted global pause stops all firing while the Daemon, API, and UI stay up.
_Avoid_: disabled

**Daemon**:
The single long-lived process that is the scheduler, the runner, the API, and the UI. The OS supervises it and schedules nothing.
_Avoid_: server, service

**Project**:
A directory registered with the Daemon and scanned for Task files. Carries a unique name — defaulting to its basename, overridable in config — that forms the first half of every Task id.
_Avoid_: watched directory, label, workspace

**Ready**:
A Task whose definition parses, validates, and names an Agent that exists. The opposite of Broken; the only state from which a Tick can produce a Run.
_Avoid_: valid, healthy, ok

**Familiar**:
A Task whose definition is the one that last started a Run. A Task that has never run, or whose file has changed since it did, is flagged instead — registering a Project trusts its committers, so an arriving or edited Task is announced rather than blocked.
_Avoid_: known, trusted, approved

**Broken**:
A Task whose file exists but whose definition fails to parse or validate — a bad schedule, a missing description, an Agent that isn't configured. Always surfaced visibly with its error; never silently unscheduled.
_Avoid_: invalid, errored
