# OpenRoutine

The open-source, local-first alternative to Claude Code Routines and ChatGPT scheduled tasks: one machine-global daemon runs markdown-defined tasks through any AI agent CLI, and is at once the scheduler, the API, and the UI.

## Language

**Task**:
A unit of agent work, defined entirely by a single markdown file — frontmatter for metadata, body as the prompt — and Registered with the Daemon by path. Its `name` is prose, written for people; its Id is derived from that name and is what everything else keys by. Moving or renaming the file changes nothing; changing `name:` to something that derives a different Id creates a new Task.
_Avoid_: routine, job, cron job

**Id**:
A Task's identity, derived from its `name` by lowercasing, turning runs of whitespace into single hyphens, dropping everything that is not a letter, digit, underscore, or hyphen, and trimming the ends. Unique across the machine — two names deriving one Id is refused, not resolved. It is what the state file records, what `runs/<id>/` is called, what an API path carries, and what the CLI answers to.
_Avoid_: slug, key, task name

**Registered**:
The state of a Task file the Daemon has been told about: one entry in the config's task list, added and removed explicitly, never discovered. Registration is the trust boundary — registering a file means trusting whoever can edit it.
_Avoid_: watched, discovered, tracked

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
A Tick that was not run, recorded with its reason: the previous Run was still active (`overlap`), the Daemon was down when the Tick passed (`daemon-down`), the Daemon was running but reached the Tick too late (`missed`), the Task was held (`paused`), or a Refresh found the Tick no longer answerable — the definition stopped loading, switched itself off, renamed itself, or a One-shot's moment moved before it arrived (`definition-changed`). Never silent — every Tick becomes exactly one Run or one Skip.
_Avoid_: dropped run, lost tick

**Idle timeout**:
How long a Run may go without its Agent saying anything. Measured from the last output rather than from the start, so work that is visibly progressing is never interrupted however long it takes, while a Run that has hung is reaped instead of held forever. When it expires the Run's whole process group is ended and the Run is recorded as timed out — never left hanging. There is deliberately no cap on a Run's total length.
_Avoid_: timeout, deadline, TTL, budget

**Jitter**:
The deterministic offset between a Task's Tick and the moment it actually fires, derived from the Task id so it never changes between runs. Spreads a machine full of midnight Tasks without making any of them unpredictable.
_Avoid_: splay, randomization, fuzz

**Fire**:
Starting a Run on demand — via the API or UI — outside the schedule.
_Avoid_: trigger, manual invocation

**Dry run**:
Rendering a Task's fully resolved execution plan — command line, environment, working directory, upcoming fire times — without spawning anything. A read, not a Run; nothing is recorded.
_Avoid_: exec, preview, test run

**One-shot**:
A Task with no `cron`. With `at:` it fires once at that timestamp; with neither it fires once, as soon as the Daemon next takes its definition in. Either way it then becomes Completed.
_Avoid_: one-off, one-time task, manual task

**Completed**:
The state of a One-shot Task that has answered its moment — by running, or by having the moment pass unattended without `catch_up`. Machine state only: the file is untouched, and editing the definition gives the Task something to do again — at the next Reload.
_Avoid_: done, finished, expired

**Catch-up**:
Opting a Task into running one Tick it missed while the Daemon was away, if that Tick is recent enough to still be wanted. Off by default; the rest of the missed Ticks stay recorded as Skips either way.
_Avoid_: backfill, replay

**Disabled**:
A Task switched off in its own definition (frontmatter). Part of the reviewable, diffable file, and not scheduled at all — so it accrues no Ticks and no Skips, unlike a Paused Task.
_Avoid_: paused

**Paused**:
A Task held at runtime, or every Task at once. Machine-owned state, never part of the definition; a Task runs only when neither Disabled nor Paused. A global pause stops all firing while the Daemon, API, and UI stay up, and survives a restart. A Tick that arrives while held becomes a Skip, not a gap.
_Avoid_: disabled, enabled

**Interrupted**:
The state of a Run whose Daemon disappeared before it finished. Recorded on the next start, under the daemon lock, so no Run is left claiming to be running forever.
_Avoid_: orphaned, crashed, stale

**Daemon**:
The single long-lived process that is the scheduler, the runner, the API, and the UI. The OS supervises it and schedules nothing.
_Avoid_: server, service

**Reload**:
The moment the Daemon re-reads its config and every Registered Task file, whether or not anything changed: at startup, on the CLI command, or on the API endpoint. `add` and `remove` request one automatically. The only way a Task that is not going to Run — Completed, Disabled, or Broken — ever notices that it was edited.
_Avoid_: rescan, hot reload

**Refresh**:
The check a Task makes on its own file at the moment it is about to Run, whether by Tick or by Fire: a changed mtime, then a changed digest, and only then the new definition, adopted for that Run. Confined to the run path and to one file — nothing is watched, nothing is polled, and a Task that never fires never Refreshes. A definition that changed into something unrunnable withdraws instead: the Run is skipped and the Task holds no schedule until a Reload says what it became.
_Avoid_: reload, rescan, watch, poll

**Ready**:
A Task whose definition parses, validates, and names an Agent that exists. The opposite of Broken; the only state from which a Tick can produce a Run.
_Avoid_: valid, healthy, ok

**Familiar**:
A Task whose definition is the one that last started a Run. A Task that has never run, or whose file has changed since it did, is flagged instead — registering a file trusts whoever can edit it, so an arriving or edited Task is announced rather than blocked.
_Avoid_: known, trusted, approved

**Broken**:
A Registered Task whose file is missing or unreadable, or whose definition fails to parse or validate — a bad schedule, a missing name or description, an empty prompt body, an Agent that isn't configured. Always surfaced visibly with its error; never silently unscheduled or unregistered.
_Avoid_: invalid, errored
