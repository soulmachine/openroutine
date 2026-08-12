# Task files are registered individually, and re-read on reload or at fire time

OpenRoutine registers task files one at a time (`openroutine add <file.md>` appends the path
to a flat `tasks = [...]` list in config). The daemon re-reads its config and every registered
file at startup and on an explicit reload — `openroutine reload`, `POST /v1/reload`, or the
reload that `add`/`remove` request automatically — and, separately, each task re-reads *its
own* file at the moment it is about to run (see "Refresh" below). There is no directory
registration, no recursive scanning, no filename convention, no file watcher, and no rescan
interval.

This supersedes the registration half of [ADR-0001](0001-machine-global-daemon.md): the
machine-global daemon stands, but "repos are registered as projects" does not. It was decided
after real use showed the unit of intent is the file, not the directory: the scanning
machinery (suffix rules, gitignore handling, symlink rules, watcher debounce) existed only to
answer "which files did the user mean?" — a question explicit registration answers exactly.

## Considered Options

- **Directory registration + scanning** (v1, replaced) — register a repo, discover
  `*.cron.md` recursively, watch for changes. Carries a discovery apparatus whose every rule
  is a guess about intent, and lets a `git pull` schedule work implicitly.
- **Managed store** — copy files into a directory OpenRoutine owns. Rejected: the in-repo
  source silently diverges from the live copy, and the daemon would still need either a scan
  or a registry to enumerate it.
- **Explicit per-file registry** (chosen) — the config lists exactly the files that are
  Tasks; files stay where the user wrote them; `remove` unregisters and deletes only under
  `--delete`.
- For change pickup: **watch/poll** (v1: notify watcher + 30s rescan) versus **explicit
  reload** (chosen). No timer and no watcher: the running schedule does not change between
  runs, and nothing on disk is consulted on a cadence.
- Within that, for the gap explicit-only left — running a prompt the author already fixed —
  three options: **poll every registered file on a timer** (rejected: the watcher by another
  name), **mtime purely as an optimisation inside an explicit reload** (rejected: a full
  reload of a handful of small files is already sub-millisecond, so it buys nothing), and a
  **Refresh on the run path** (chosen): a task stats its own file when it is about to run,
  and adopts the change if there is one. mtime is only a pre-filter — a changed mtime causes
  a read, and only a changed content digest counts — because `git checkout` and `touch` move
  mtime without changing content, and re-running every cron-less one-shot on a branch switch
  would spend real agent calls for nothing.

## Consequences

- Task identity is an **Id derived from** the mandatory frontmatter `name` (DECISIONS Q88):
  lowercase, whitespace runs to single hyphens, anything outside `[a-z0-9_-]` dropped, ends
  trimmed — so the name stays readable prose while the Id is always safe as a directory under
  `runs/` and a path segment in the API. Flat and machine-unique — no more
  `<project>/<filename stem>`. Moving or renaming a file changes nothing; editing `name:` into
  something that derives a different Id creates a new Task. Uniqueness is checked on the Id,
  never the name: `add` refuses a duplicate outright, and on Reload config order decides —
  first wins, the later file is Broken.
- The API uses single-segment task paths (`/v1/tasks/{id}`, `/v1/runs/{id}/{run}`), run
  history lives at `runs/<id>/`, and state entries key on the flat Id and always carry
  the resolved `cwd`. Nothing migrates: a v1 `[[projects]]` config gets a friendly error, and
  old run directories are left orphaned.
- `cwd` resolves against the task file's own directory (absolute paths allowed, default is
  that directory) — the project root no longer exists to resolve against, and the
  inside-the-project restriction went with it. Trust attaches to registering the file.
- Registration is the trust boundary: registering a file means trusting whoever can edit it.
- The Project and CRONTAB.md concepts are gone, along with the `ignore`/`notify`
  dependencies, the suffix rule, and the unreachable-project bookkeeping.
- **Refresh reaches only tasks that still fire.** A completed one-shot, a `disabled: true`
  task, and a broken task never come due, so none of them can notice an edit on its own —
  `openroutine reload` is the only route. Flipping `disabled` back to `false` in a file is
  the trap this creates, and is documented next to the field.
- The fire moment gains rules, because a definition can change out from under a tick that was
  already planned. They fall into two kinds. A definition that became unrunnable — broken,
  newly disabled, or renamed — **withdraws**: the run is skipped and the task drops out of the
  schedule until a reload says what it became, since none of those is a state the run path can
  settle. A one-shot whose moment moved is only **re-armed**: that tick is skipped and the task
  stays scheduled at its new moment. A cron tick absent from the new schedule still runs. All
  record a `definition-changed` skip, so every tick still becomes exactly one run or one skip.
- What `list`, `status`, and the UI show still comes from disk or from the last reload, so a
  refreshed definition is not visible there until something reads the file again.
