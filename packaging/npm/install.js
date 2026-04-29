#!/usr/bin/env node
// Downloads the platform-specific betterhook binary from GitHub Releases
// and places it at bin/betterhook-native. Runs as a postinstall script.
//
// The binary ships as a .tar.gz archive. After download we extract it,
// verify the file is executable, and clean up the archive.

const https = require("https");
const http = require("http");
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

function getPlatformKey() {
  const key = `${process.platform}-${process.arch}`;
  const asset = PLATFORM_MAP[key];
  if (!asset) {
    console.error(
      `betterhook: unsupported platform ${key}. ` +
        `Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`
    );
    process.exit(1);
  }
  return asset;
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const follow = (url) => {
      const client = url.startsWith("https") ? https : http;
      client
        .get(url, { headers: { "User-Agent": "betterhook-npm" } }, (res) => {
          if (
            res.statusCode >= 300 &&
            res.statusCode < 400 &&
            res.headers.location
          ) {
            follow(res.headers.location);
            return;
          }
          if (res.statusCode !== 200) {
            reject(new Error(`HTTP ${res.statusCode} from ${url}`));
            return;
          }
          const file = fs.createWriteStream(dest);
          res.pipe(file);
          file.on("finish", () => file.close(resolve));
        })
        .on("error", reject);
    };
    follow(url);
  });
}

async function main() {
  const asset = getPlatformKey();
  const archiveName = `${asset}.tar.gz`;
  const url = `${BASE_URL}/${archiveName}`;
  const binDir = path.join(__dirname, "bin");
  const archivePath = path.join(binDir, archiveName);
  const nativeBinary = path.join(binDir, "betterhook-native");

  fs.mkdirSync(binDir, { recursive: true });

  console.log(`betterhook: downloading ${archiveName}`);
  await download(url, archivePath);

  // Extract the binary from the tar.gz archive.
  // The archive contains a single file named after the asset.
  execSync(`tar xzf "${archivePath}" -C "${binDir}"`, { stdio: "pipe" });

  // Rename the extracted binary to a stable name.
  const extracted = path.join(binDir, asset);
  if (fs.existsSync(extracted)) {
    fs.renameSync(extracted, nativeBinary);
  } else {
    // Fallback: if tar extracted with a different structure, find it.
    console.error(`betterhook: expected ${extracted} after extraction`);
    process.exit(1);
  }

  fs.chmodSync(nativeBinary, 0o755);

  // Clean up the archive.
  fs.unlinkSync(archivePath);

  console.log(`betterhook: installed to ${nativeBinary}`);
}

main().catch((err) => {
  console.error(`betterhook: install failed: ${err.message}`);
  console.error(
    "You can install betterhook manually from https://github.com/leonardomso/betterhook/releases"
  );
  process.exit(1);
});
