// Fetches the prebuilt binary for this platform from the GitHub release that
// matches this package's version. The binary lands in dist/, where the bin
// shim expects it. No dependencies: fetch and tar are enough.
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");
const { version } = require("./package.json");

const TARGETS = {
  "darwin arm64": "aarch64-apple-darwin",
  "darwin x64": "x86_64-apple-darwin",
  "linux x64": "x86_64-unknown-linux-gnu",
  "linux arm64": "aarch64-unknown-linux-gnu",
};

function fail(message) {
  console.error(`openroutine: ${message}`);
  console.error(
    "openroutine: you can always build from source instead: cargo install openroutine"
  );
  process.exit(1);
}

async function main() {
  const key = `${process.platform} ${process.arch}`;
  const target = TARGETS[key];
  if (!target) fail(`no prebuilt binary for ${key}`);

  const asset = `openroutine-v${version}-${target}.tar.gz`;
  const url = `https://github.com/soulmachine/openroutine/releases/download/v${version}/${asset}`;

  const res = await fetch(url);
  if (!res.ok) fail(`download failed: ${res.status} ${res.statusText} for ${url}`);

  const tarball = path.join(os.tmpdir(), asset);
  fs.writeFileSync(tarball, Buffer.from(await res.arrayBuffer()));

  const dist = path.join(__dirname, "dist");
  fs.mkdirSync(dist, { recursive: true });
  execFileSync("tar", ["-xzf", tarball, "-C", dist]);
  fs.rmSync(tarball);

  const binary = path.join(dist, "openroutine");
  fs.accessSync(binary, fs.constants.X_OK);
}

main().catch((err) => fail(err.message));
