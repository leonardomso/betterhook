#!/usr/bin/env node
// Downloads the platform-specific betterhook binary from GitHub Releases
// and places it at bin/betterhook-native. Runs as a postinstall script.
//
// The binary ships as a .tar.gz archive. We extract it in-process using
// Node's built-in zlib + tar header parsing (no external `tar` binary).

const https = require("https");
const fs = require("fs");
const path = require("path");
const zlib = require("zlib");

const VERSION = require("./package.json").version;
const BASE_URL = `https://github.com/leonardomso/betterhook/releases/download/v${VERSION}`;

const PLATFORM_MAP = {
  "darwin-arm64": "betterhook-aarch64-apple-darwin",
  "darwin-x64": "betterhook-x86_64-apple-darwin",
  "linux-arm64": "betterhook-aarch64-unknown-linux-gnu",
  "linux-x64": "betterhook-x86_64-unknown-linux-gnu",
};

// Maximum redirects to follow (GitHub typically uses 1-2).
const MAX_REDIRECTS = 5;

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
    let redirects = 0;
    const follow = (currentUrl) => {
      if (!currentUrl.startsWith("https://")) {
        reject(new Error(`refusing non-HTTPS redirect: ${currentUrl}`));
        return;
      }
      if (redirects++ > MAX_REDIRECTS) {
        reject(new Error(`too many redirects (>${MAX_REDIRECTS})`));
        return;
      }
      https
        .get(currentUrl, { headers: { "User-Agent": "betterhook-npm" } }, (res) => {
          if (
            res.statusCode >= 300 &&
            res.statusCode < 400 &&
            res.headers.location
          ) {
            // Resolve relative redirects against the current URL.
            const next = new URL(res.headers.location, currentUrl).href;
            follow(next);
            return;
          }
          if (res.statusCode !== 200) {
            reject(new Error(`HTTP ${res.statusCode} from ${currentUrl}`));
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

// Minimal tar extractor: reads a .tar.gz and extracts the first regular
// file. betterhook release archives contain exactly one file.
function extractFirstFile(archivePath, outputPath) {
  return new Promise((resolve, reject) => {
    const input = fs.createReadStream(archivePath);
    const gunzip = zlib.createGunzip();
    const chunks = [];

    gunzip.on("data", (chunk) => chunks.push(chunk));
    gunzip.on("end", () => {
      const tar = Buffer.concat(chunks);
      // Tar header: first 512 bytes. File name at offset 0 (100 bytes),
      // size at offset 124 (12 bytes, octal, NUL-terminated).
      if (tar.length < 512) {
        reject(new Error("tar archive too small"));
        return;
      }
      const sizeStr = tar.subarray(124, 136).toString("ascii").replace(/\0/g, "").trim();
      const fileSize = parseInt(sizeStr, 8);
      if (isNaN(fileSize) || fileSize <= 0) {
        reject(new Error(`invalid tar entry size: '${sizeStr}'`));
        return;
      }
      if (tar.length < 512 + fileSize) {
        reject(new Error("tar archive truncated"));
        return;
      }
      fs.writeFileSync(outputPath, tar.subarray(512, 512 + fileSize));
      resolve();
    });
    gunzip.on("error", reject);
    input.on("error", reject);
    input.pipe(gunzip);
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

  // Extract in-process (no external tar dependency).
  await extractFirstFile(archivePath, nativeBinary);
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
