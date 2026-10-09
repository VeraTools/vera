#!/usr/bin/env node

"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const fsp = require("node:fs/promises");
const http = require("node:http");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const { pipeline } = require("node:stream/promises");
const { Transform } = require("node:stream");
const zlib = require("node:zlib");

const { version: packageVersion } = require("../package.json");

const DEFAULT_REPO = "VeraTools/Vera";
const MAX_REDIRECTS = 5;
const REQUEST_TIMEOUT = 30_000;
const DOWNLOAD_TIMEOUT = 180_000;
const MAX_MANIFEST_BYTES = 1024 * 1024;
const MAX_ARCHIVE_BYTES = 512 * 1024 * 1024;
const MAX_BINARY_BYTES = 512 * 1024 * 1024;
const VERSION_PATTERN = /^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$/;

function parseArgs(argv) {
  if (argv.length === 0) {
    return { command: "help", rest: [] };
  }

  return { command: argv[0], rest: argv.slice(1) };
}

function detectMusl() {
  if (process.platform !== "linux") return false;
  const result = spawnSync("ldd", ["--version"], {
    stdio: ["pipe", "pipe", "pipe"], timeout: 5000,
  });
  const output = (result.stdout || "").toString() + (result.stderr || "").toString();
  if (/musl/i.test(output)) return true;
  if (!result.error && /glibc|GNU libc/i.test(output)) return false;
  try {
    return fs.readdirSync("/lib").some((entry) => entry.startsWith("ld-musl-"));
  } catch {
    return false;
  }
}

function resolveTarget(platform, arch) {
  const override = process.env.VERA_TARGET;
  if (override) return override;

  const key = `${platform}:${arch}`;
  const targets = {
    "linux:x64": detectMusl()
      ? "x86_64-unknown-linux-musl"
      : "x86_64-unknown-linux-gnu",
    "linux:arm64": "aarch64-unknown-linux-gnu",
    "darwin:x64": "x86_64-apple-darwin",
    "darwin:arm64": "aarch64-apple-darwin",
    "win32:x64": "x86_64-pc-windows-msvc",
  };

  const target = targets[key];
  if (!target) {
    throw new Error(`unsupported platform: ${platform}/${arch}`);
  }

  return target;
}

function defaultReleaseBaseUrl() {
  return process.env.VERA_RELEASE_BASE_URL || `https://github.com/${DEFAULT_REPO}`;
}

function manifestUrl(version) {
  if (process.env.VERA_MANIFEST_URL) {
    return process.env.VERA_MANIFEST_URL;
  }

  return `${defaultReleaseBaseUrl()}/releases/download/v${version}/release-manifest.json`;
}

function defaultVeraHome() {
  if (process.env.VERA_HOME && process.env.VERA_HOME.trim()) return path.resolve(process.env.VERA_HOME);
  const home = os.homedir();
  const legacy = path.join(home, ".vera");
  try {
    if (fs.readdirSync(legacy).some((name) => !name.startsWith(".") && name !== "update-check.json")) return legacy;
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const data = process.platform === "win32" ? process.env.APPDATA : process.platform === "darwin"
    ? path.join(home, "Library", "Application Support")
    : process.env.XDG_DATA_HOME && path.isAbsolute(process.env.XDG_DATA_HOME)
      ? process.env.XDG_DATA_HOME : path.join(home, ".local", "share");
  return data ? path.join(data, "vera") : legacy;
}

function installMetadataPath() {
  return path.join(defaultVeraHome(), "install.json");
}

function currentInstallMethod() {
  if (process.versions.bun) return "bun";
  const ua = process.env.npm_config_user_agent || "";
  if (ua.startsWith("bun/")) return "bun";
  const execpath = process.env.npm_execpath || "";
  if (execpath.includes("bun")) return "bun";
  return "npm";
}

async function readInstallMetadata() {
  try {
    const raw = await fsp.readFile(installMetadataPath(), "utf8");
    const value = JSON.parse(raw);
    return value && typeof value === "object" && !Array.isArray(value) ? value : {};
  } catch {
    return {};
  }
}

async function writeInstallMetadata({ installMethod, version, binaryPath, target }) {
  const metadataPath = installMetadataPath();
  await fsp.mkdir(path.dirname(metadataPath), { recursive: true });
  const current = await readInstallMetadata();
  const next = {
    install_method: installMethod ?? current.install_method ?? null,
    version: version ?? current.version ?? null,
    binary_path: binaryPath ?? current.binary_path ?? null,
    target: target ?? current.target ?? null,
    manifest_url: process.env.VERA_MANIFEST_URL || null,
    requested_version: packageVersion,
  };
  const tmpPath = `${metadataPath}.tmp.${process.pid}`;
  await fsp.writeFile(tmpPath, `${JSON.stringify(next, null, 2)}\n`, "utf8");
  await fsp.rename(tmpPath, metadataPath);
}

function preferredBinDirs() {
  const home = os.homedir();
  if (process.env.VERA_USER_BIN_DIR) {
    return [process.env.VERA_USER_BIN_DIR];
  }

  if (process.platform === "win32") {
    return [
      path.join(home, "AppData", "Roaming", "npm"),
      path.join(home, "AppData", "Local", "Programs", "Vera", "bin"),
    ];
  }

  return [
    path.join(home, ".local", "bin"),
    path.join(home, ".cargo", "bin"),
    path.join(home, "bin"),
  ];
}

function pathEntries() {
  return (process.env.PATH || "")
    .split(path.delimiter)
    .filter(Boolean)
    .map((entry) => path.resolve(entry));
}

function pickUserBinDir() {
  const entries = new Set(pathEntries());
  const candidates = preferredBinDirs().map((entry) => path.resolve(entry));
  return candidates.find((entry) => entries.has(entry)) || candidates[0];
}

function binaryName() {
  return process.platform === "win32" ? "vera.exe" : "vera";
}

function shimName() {
  return process.platform === "win32" ? "vera.cmd" : "vera";
}

function openResponse(url, timeout = REQUEST_TIMEOUT, redirects = 0, deadline = Date.now() + timeout) {
  const parsed = new URL(url);
  if (!["http:", "https:"].includes(parsed.protocol)) {
    throw new Error("release downloads require an HTTP or HTTPS URL");
  }
  return new Promise((resolve, reject) => {
    const remaining = deadline - Date.now();
    if (remaining <= 0) return reject(new Error("release download timed out"));
    const client = parsed.protocol === "https:" ? https : http;
    const request = client.get(parsed, (response) => {
      const status = response.statusCode || 0;
      if ([301, 302, 303, 307, 308].includes(status) && response.headers.location) {
        if (redirects >= MAX_REDIRECTS) {
          reject(new Error("too many release redirects"));
        } else {
          resolve(Promise.resolve().then(() => openResponse(new URL(response.headers.location, parsed).toString(), timeout, redirects + 1, deadline)));
        }
        response.destroy();
        return;
      }
      if (status < 200 || status >= 300) {
        response.destroy();
        reject(new Error(`request failed for ${url}: ${status}`));
        return;
      }
      resolve(response);
    });
    const timer = setTimeout(() => request.destroy(new Error("release download timed out")), remaining);
    request.on("close", () => clearTimeout(timer));
    request.on("error", reject);
  });
}

async function fetchText(url, timeout = REQUEST_TIMEOUT) {
  const response = await openResponse(url, timeout);
  const chunks = [];
  let size = 0;
  for await (const chunk of response) {
    size += chunk.length;
    if (size > MAX_MANIFEST_BYTES) {
      response.destroy();
      throw new Error("release manifest exceeds its size limit");
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function downloadFile(url, destination, expectedSize, timeout = DOWNLOAD_TIMEOUT) {
  let size = 0;
  try {
    const response = await openResponse(url, timeout);
    const bound = new Transform({
      transform(chunk, encoding, callback) {
        size += chunk.length;
        callback(size > expectedSize ? new Error("release archive exceeds its size limit") : null, chunk);
      },
    });
    await pipeline(response, bound, fs.createWriteStream(destination, { flags: "wx" }));
    if (size !== expectedSize) throw new Error("release archive size mismatch");
  } catch (error) {
    await fsp.rm(destination, { force: true });
    throw error;
  }
}

async function sha256(filePath) {
  const hash = crypto.createHash("sha256");
  const stream = fs.createReadStream(filePath);

  return new Promise((resolve, reject) => {
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("end", () => resolve(hash.digest("hex")));
    stream.on("error", reject);
  });
}

function safeMember(name) {
  return name && !name.startsWith("/") && !/[\\:\x00]/.test(name) &&
    name.replace(/\/$/, "").split("/").every((part) => part && part !== "." && part !== "..");
}

async function extractArchive(archivePath, destination, target) {
  const expected = `vera-${target}/${binaryName()}`;
  if (archivePath.endsWith(".zip")) {
    // .NET reads only the selected entry; archive paths never become filesystem paths.
    const script = `
      $ErrorActionPreference = 'Stop'
      Add-Type -AssemblyName System.IO.Compression.FileSystem
      $archive = [System.IO.Compression.ZipFile]::OpenRead($env:VERA_INSTALL_ARCHIVE)
      try {
        $matches = @()
        $total = 0
        $names = @{}
        if ($archive.Entries.Count -gt 1024) { throw 'Too many release archive members' }
        foreach ($entry in $archive.Entries) {
          $name = $entry.FullName
          $type = ($entry.ExternalAttributes -shr 16) -band 61440
          $parts = $name.TrimEnd('/').Split('/')
          if ($names.ContainsKey($name) -or !$name -or $name.StartsWith('/') -or $name.Contains('\\') -or $name.Contains(':') -or
              ($parts | Where-Object { $_ -eq '..' -or $_ -eq '.' -or $_ -eq '' }) -or
              ($type -ne 0 -and $type -ne 32768 -and $type -ne 16384)) { throw 'Unsafe release archive member' }
          $names[$name] = $true
          $total += $entry.Length
          if ($total -gt ${MAX_BINARY_BYTES}) { throw 'Release archive exceeds its size limit' }
          if ($name -ceq $env:VERA_INSTALL_MEMBER) {
            if ($type -eq 16384) { throw 'Release binary must be a regular file' }
            $matches += $entry
          }
        }
        if ($matches.Count -ne 1 -or $matches[0].Length -le 0) { throw 'Release archive must contain exactly one Vera binary' }
        $source = $matches[0].Open()
        $output = [System.IO.File]::Open($env:VERA_INSTALL_BINARY, [System.IO.FileMode]::CreateNew)
        try {
          $buffer = New-Object byte[] 65536
          $copied = 0
          while (($count = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $copied += $count
            if ($copied -gt $matches[0].Length -or $copied -gt ${MAX_BINARY_BYTES}) { throw 'Release binary exceeds its size limit' }
            $output.Write($buffer, 0, $count)
          }
        } finally { $source.Dispose(); $output.Dispose() }
        if ((Get-Item -LiteralPath $env:VERA_INSTALL_BINARY).Length -ne $matches[0].Length) { throw 'Incomplete release binary' }
      } finally { $archive.Dispose() }
    `;
    const result = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
      stdio: "inherit", timeout: 30_000,
      env: { ...process.env, VERA_INSTALL_ARCHIVE: archivePath, VERA_INSTALL_BINARY: destination, VERA_INSTALL_MEMBER: expected },
    });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error("release ZIP extraction failed");
    return;
  }

  const output = await fsp.open(destination, "wx");
  const input = fs.createReadStream(archivePath);
  const gunzip = zlib.createGunzip();
  input.on("error", (error) => gunzip.destroy(error));
  input.pipe(gunzip);
  let buffer = Buffer.alloc(0);
  let remaining = 0;
  let padding = 0;
  let selected = false;
  let matches = 0;
  let expanded = 0;
  let members = 0;
  let ended = false;
  const names = new Set();
  try {
    for await (const chunk of gunzip) {
      expanded += chunk.length;
      if (expanded > MAX_BINARY_BYTES + 1024 * 1024) throw new Error("release archive exceeds its size limit");
      buffer = Buffer.concat([buffer, chunk]);
      while (buffer.length) {
        if (remaining || padding) {
          const count = Math.min(buffer.length, remaining || padding);
          if (remaining) {
            if (selected) await output.writeFile(buffer.subarray(0, count));
            remaining -= count;
          } else padding -= count;
          buffer = buffer.subarray(count);
          continue;
        }
        if (buffer.length < 512) break;
        const header = buffer.subarray(0, 512);
        buffer = buffer.subarray(512);
        if (header.every((byte) => byte === 0)) { ended = true; continue; }
        if (ended || ++members > 1024) throw new Error("invalid release tar archive");
        const field = (start, length) => header.subarray(start, start + length).toString("utf8").split("\0")[0];
        const octal = (start, length) => {
          const value = field(start, length).trim();
          if (!/^[0-7]+$/.test(value)) throw new Error("invalid release tar header");
          return parseInt(value, 8);
        };
        const checksum = octal(148, 8);
        const actual = header.reduce((sum, byte, index) => sum + (index >= 148 && index < 156 ? 32 : byte), 0);
        if (actual !== checksum) throw new Error("invalid release tar checksum");
        const prefix = field(345, 155);
        const name = (prefix ? `${prefix}/` : "") + field(0, 100);
        const type = String.fromCharCode(header[156]);
        if (names.has(name) || !safeMember(name) || !["0", "\0", "5"].includes(type)) throw new Error("unsafe release archive member");
        names.add(name);
        remaining = octal(124, 12);
        if (remaining > MAX_BINARY_BYTES || (type === "5" && remaining)) throw new Error("invalid release tar size");
        padding = (512 - remaining % 512) % 512;
        selected = name === expected && type !== "5";
        if (selected && (++matches !== 1 || remaining === 0)) throw new Error("release archive must contain exactly one Vera binary");
      }
    }
    if (remaining || padding || buffer.some((byte) => byte !== 0) || !ended || matches !== 1) {
      throw new Error("incomplete release tar archive or missing Vera binary");
    }
  } finally {
    input.destroy();
    gunzip.destroy();
    await output.close();
  }
}

function shimContents(binaryPath, platform) {
  if (platform === "win32") {
    return `@echo off\r\nsetlocal DisableDelayedExpansion\r\n"${binaryPath.replace(/%/g, "%%")}" %*\r\n`;
  }
  return `#!/bin/sh\nexec '${binaryPath.replace(/'/g, "'\"'\"'")}' "$@"\n`;
}

function shimTarget(text) {
  if (text.startsWith("#!/bin/sh\nexec ") && text.endsWith(' "$@"\n')) {
    const word = text.slice(15, -6);
    if (word.startsWith("'") && word.endsWith("'")) {
      const parts = word.slice(1, -1).split("'\"'\"'");
      if (parts.every((part) => !part.includes("'"))) return parts.join("'") || null;
    } else if (word.startsWith('"') && word.endsWith('"')) {
      const target = word.slice(1, -1);
      return target && !/[$`\\"]/.test(target) ? target : null;
    } else if (/^[A-Za-z0-9@%+=:,./_-]+$/.test(word)) return word;
  }
  for (const setlocal of [true, false]) {
    const head = '@echo off\r\n' + (setlocal ? 'setlocal DisableDelayedExpansion\r\n' : '') + '"';
    if (text.startsWith(head) && text.endsWith('" %*\r\n')) {
      const target = text.slice(head.length, -6);
      // cmd expands a lone `%`; only the escaped form may carry a literal one.
      if (target.includes('"') || (setlocal ? target.replace(/%%/g, "") : target).includes("%")) return null;
      return (setlocal ? target.replace(/%%/g, "%") : target) || null;
    }
  }
  return null;
}

async function createShim(binaryPath) {
  const binDir = pickUserBinDir();
  await fsp.mkdir(binDir, { recursive: true });
  const shimPath = path.join(binDir, shimName());
  if (path.join(await fsp.realpath(binDir), shimName()) === await fsp.realpath(binaryPath)) return shimPath;
  const contents = shimContents(binaryPath, process.platform);
  try {
    const stat = await fsp.lstat(shimPath);
    const current = stat.isFile() ? await fsp.readFile(shimPath, "utf8").catch(() => "") : "";
    const target = shimTarget(current);
    const normalized = target && path.isAbsolute(target) ? path.resolve(target) : null;
    // Releases live at <home>/bin/<version>/<target>/<binary>. The legacy home
    // counts too: an older uninstall could leave its shim behind.
    const owned = normalized && [defaultVeraHome(), path.join(os.homedir(), ".vera")].some((home) => {
      const relative = path.relative(path.resolve(home, "bin"), normalized);
      const parts = relative.split(path.sep);
      return !path.isAbsolute(relative) && parts.length === 3 && parts[0] !== ".." && parts[2] === binaryName();
    });
    if (!owned) {
      console.error(`Left ${shimPath} in place because Vera did not create it. Run ${binaryPath} directly, or remove that file and install again.`);
      return null;
    }
    if (current === contents) return shimPath;
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const staging = await fsp.mkdtemp(path.join(binDir, ".vera-shim-"));
  const stagedShim = path.join(staging, shimName());
  try {
    await fsp.writeFile(stagedShim, contents, { mode: 0o755 });
    await fsp.rename(stagedShim, shimPath);
  } finally {
    await fsp.rm(staging, { recursive: true, force: true });
  }
  return shimPath;
}

function isOnPath(dirPath) {
  return pathEntries().includes(path.resolve(dirPath));
}

async function loadManifest() {
  const manifest = JSON.parse(await fetchText(manifestUrl(packageVersion)));
  if (!manifest || typeof manifest.version !== "string" || !VERSION_PATTERN.test(manifest.version)) {
    throw new Error("invalid release version");
  }
  if (!process.env.VERA_MANIFEST_URL && manifest.version !== packageVersion) {
    throw new Error(`requested Vera ${packageVersion}, manifest contains ${manifest.version}`);
  }
  return manifest;
}

async function cachedBinary(target) {
  let version = packageVersion;
  if (process.env.VERA_MANIFEST_URL) {
    const metadata = await readInstallMetadata();
    if (metadata.manifest_url !== process.env.VERA_MANIFEST_URL || metadata.requested_version !== packageVersion) return null;
    version = metadata.version;
    if (typeof version !== "string" || !VERSION_PATTERN.test(version)) return null;
  }
  const binaryPath = path.join(defaultVeraHome(), "bin", version, target, binaryName());
  try {
    const receipt = JSON.parse(await fsp.readFile(`${binaryPath}.receipt.json`, "utf8"));
    const stat = await fsp.lstat(binaryPath);
    if (receipt && stat.isFile() && stat.size > 0 && receipt.version === version && receipt.target === target &&
        receipt.size === stat.size && receipt.sha256 === await sha256(binaryPath)) {
      return { binaryPath, version };
    }
  } catch (error) {
    if (!["ENOENT", "ENOTDIR"].includes(error.code) && !(error instanceof SyntaxError)) throw error;
  }
  return null;
}

async function finishInstall(binaryPath, version, target, install, downloaded = false) {
  const writeLauncher = install || !fs.lstatSync(path.join(pickUserBinDir(), shimName()), { throwIfNoEntry: false });
  if (writeLauncher) {
    const shimPath = await createShim(binaryPath);
    if (downloaded && shimPath && !isOnPath(path.dirname(shimPath))) {
      console.error(`Added Vera to ${path.dirname(shimPath)}. Add that directory to PATH to run \`vera\` directly.`);
    }
  }
  if (writeLauncher || downloaded) await writeInstallMetadata({ installMethod: currentInstallMethod(), version, binaryPath, target });
  return { binaryPath, version };
}

async function ensureBinaryInstalled(install = false) {
  const target = resolveTarget(process.platform, process.arch);
  if (!/^[A-Za-z0-9_-]+$/.test(target)) throw new Error("invalid release target");
  const cached = await cachedBinary(target);
  if (cached) return finishInstall(cached.binaryPath, cached.version, target, install);
  const manifest = await loadManifest();
  const asset = manifest.assets && manifest.assets[target];
  if (!asset) throw new Error(`no release asset for target ${target}`);
  const extension = target.endsWith("windows-msvc") ? ".zip" : ".tar.gz";
  if (asset.archive !== `vera-${target}${extension}` || !Number.isSafeInteger(asset.size) ||
      asset.size <= 0 || asset.size > MAX_ARCHIVE_BYTES || !/^[0-9a-f]{64}$/.test(asset.sha256)) {
    throw new Error("invalid release archive metadata");
  }
  const version = manifest.version;
  const installDir = path.join(defaultVeraHome(), "bin", version, target);
  const binaryPath = path.join(installDir, binaryName());
  await fsp.mkdir(installDir, { recursive: true });
  const tempRoot = await fsp.mkdtemp(path.join(installDir, ".install-"));
  try {
    const archivePath = path.join(tempRoot, asset.archive);
    const stagedBinary = path.join(tempRoot, binaryName());
    console.error(`Downloading Vera ${version} for ${target}...`);
    await downloadFile(asset.download_url, archivePath, asset.size);
    if (await sha256(archivePath) !== asset.sha256) throw new Error(`checksum mismatch for ${asset.archive}`);
    await extractArchive(archivePath, stagedBinary, target);
    if (process.platform !== "win32") await fsp.chmod(stagedBinary, 0o755);
    const stat = await fsp.stat(stagedBinary);
    if (!stat.isFile() || stat.size <= 0 || stat.size > MAX_BINARY_BYTES) throw new Error("invalid release binary");
    const receiptPath = path.join(tempRoot, "receipt.json");
    await fsp.writeFile(receiptPath, JSON.stringify({ version, target, size: stat.size, sha256: await sha256(stagedBinary) }));
    await fsp.rename(stagedBinary, binaryPath);
    await fsp.rename(receiptPath, `${binaryPath}.receipt.json`);
  } finally {
    await fsp.rm(tempRoot, { recursive: true, force: true });
  }
  return finishInstall(binaryPath, version, target, install, true);
}

async function runBinary(binaryPath, args) {
  const child = spawn(binaryPath, args, { stdio: "inherit" });
  const signals = ["SIGINT", "SIGTERM", "SIGHUP"];
  const forward = new Map(signals.map((signal) => [signal, () => child.kill(signal)]));
  for (const [signal, handler] of forward) process.on(signal, handler);
  try {
    const result = await new Promise((resolve, reject) => {
      child.on("error", reject);
      child.on("exit", (code, signal) => resolve({ code, signal }));
    });
    for (const [signal, handler] of forward) process.removeListener(signal, handler);
    if (result.signal && process.platform !== "win32") process.kill(process.pid, result.signal);
    else process.exitCode = result.code ?? 1;
  } finally {
    for (const [signal, handler] of forward) process.removeListener(signal, handler);
  }
}

async function main() {
  const { command, rest } = parseArgs(process.argv.slice(2));

  const { binaryPath, version } = await ensureBinaryInstalled(command === "install");

  if (command === "install") {
    console.error(`Vera ${version} installed.`);
    if (rest.length === 0 && (!process.stdin.isTTY || !process.stderr.isTTY)) {
      console.error("Run `vera agent install` in a terminal or `vera agent install --client all --scope global` to install agent skills.");
      return;
    }
    await runBinary(binaryPath, ["agent", "install", ...rest]);
    return;
  }

  if (command === "help") {
    await runBinary(binaryPath, ["--help"]);
    return;
  }

  await runBinary(binaryPath, [command, ...rest]);
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

module.exports = { defaultVeraHome, shimContents, shimTarget, detectMusl, fetchText, downloadFile, extractArchive, ensureBinaryInstalled, createShim, runBinary };
