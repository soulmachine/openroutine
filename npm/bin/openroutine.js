#!/usr/bin/env node
// Hands straight off to the real binary that install.js downloaded. A shim
// rather than the binary itself, so npm's bin link exists before postinstall
// runs and works the same under every package manager.
"use strict";

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const binary = path.join(__dirname, "..", "dist", "openroutine");

if (!fs.existsSync(binary)) {
  console.error(
    "openroutine: binary missing — the install step did not run.\n" +
      "Reinstall without --ignore-scripts, or build from source: cargo install openroutine"
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
