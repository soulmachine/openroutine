# OpenRoutine

> Your AI agents' crontab, as markdown.

One markdown file per task, crontab syntax in the frontmatter, any AI coding
agent underneath — Claude Code, Codex, Gemini, your own. Tasks live in your
repo, run on your machine, and answer to no account, plan, or cap.

This package installs a prebuilt binary (macOS and Linux, x64 and arm64),
fetched from the matching [GitHub release](https://github.com/soulmachine/openroutine/releases)
at install time. Prefer building from source? `cargo install openroutine`.

```bash
npm install -g openroutine
openroutine init                # writes a config and prints a sample task
openroutine add my-task.md      # register a task file you wrote
openroutine serve               # run the scheduler in the foreground
```

Docs, the full CLI, and the REST API: [openroutine.dev](https://openroutine.dev) ·
[GitHub](https://github.com/soulmachine/openroutine)
