#!/usr/bin/env node
// Downloads the platform-specific betterhook binary from GitHub Releases
// and places it at bin/betterhook. Runs as a postinstall script.
//
// Follows the same pattern as @biomejs/biome and oxlint: thin npm
// wrapper that delegates to a native binary.

const https = require("https");
const fs = require("fs");
const path = require("path");
const { execSync } = require("child_process");

const VERSION = require("./package.json").version;
const BASE_URL = `https://github.com/leonardomso/betterhook/releases/download/v${VERSION}`;

const PLATFORM_MAP = {
  "darwin-arm64": "betterhook-aarch64-apple-darwin",
  "darwin-x64": "betterhook-x86_64-apple-darwin",
  "linux-arm64": "betterhook-aarch64-unknown-linux-gnu",
  "linux-x64": "betterhook-x86_64-unknown-linux-gnu",
};

function getPlatformBinary() {
  const key = `${process.platform}-${process.arch}`;
  const binary = PLATFORM_MAP[key];
  if (!binary) {
    console.error(
      `betterhook: unsupported platform ${key}. ` +
        `Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`
    );
    process.exit(1);
  }
  return binary;
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const follow = (url) => {
      https
        .get(url, { headers: { "User-Agent": "betterhook-npm" } }, (res) => {
          if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
            follow(res.headers.location);
            return;
          }
          if (res.statusCode !== 200) {
            reject(new Error(`HTTP ${res.statusCode} from ${url}`));
            return;
          }
          const file = fs.createWriteStream(dest);
          res.pipe(file);
          file.on("finish", () => {
            file.close(resolve);
          });
        })
        .on("error", reject);
    };
    follow(url);
  });
}

async function main() {
  const binary = getPlatformBinary();
  const url = `${BASE_URL}/${binary}`;
  const dest = path.join(__dirname, "bin", "betterhook");

  fs.mkdirSync(path.dirname(dest), { recursive: true });

  console.log(`betterhook: downloading ${url}`);
  await download(url, dest);
  fs.chmodSync(dest, 0o755);
  console.log(`betterhook: installed to ${dest}`);
}

main().catch((err) => {
  console.error(`betterhook: install failed: ${err.message}`);
  process.exit(1);
});
