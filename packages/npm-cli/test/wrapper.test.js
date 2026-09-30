"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const fsp = require("node:fs/promises");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const test = require("node:test");
const vm = require("node:vm");
const zlib = require("node:zlib");
const wrapper = require("../bin/vera.js");
const packageVersion = require("../package.json").version;
const target = "x86_64-unknown-linux-gnu";
const binaryName = process.platform === "win32" ? "vera.exe" : "vera";
const member = `vera-${target}/${binaryName}`;

function tar(entries) {
  const blocks = [];
  for (const { name, contents = "", type = "0" } of entries) {
    const body = Buffer.from(contents);
    const header = Buffer.alloc(512);
    header.write(name, 0, 100);
    header.write("0000755\0", 100);
    header.write("0000000\0", 108);
    header.write("0000000\0", 116);
    header.write(body.length.toString(8).padStart(11, "0") + "\0", 124);
    header.write("00000000000\0", 136);
    header.fill(32, 148, 156);
    header.write(type, 156);
    header.write("ustar\0", 257);
    const checksum = header.reduce((sum, byte) => sum + byte, 0);
    header.write(checksum.toString(8).padStart(6, "0") + "\0 ", 148);
    blocks.push(header, body, Buffer.alloc((512 - body.length % 512) % 512));
  }
  return zlib.gzipSync(Buffer.concat([...blocks, Buffer.alloc(1024)]));
}

async function fixture(t, handler) {
  const temp = await fsp.mkdtemp(path.join(os.tmpdir(), "vera-wrapper-test-"));
  const saved = { ...process.env };
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  Object.assign(process.env, { VERA_HOME: path.join(temp, "home"), VERA_USER_BIN_DIR: path.join(temp, "bin"), VERA_TARGET: target });
  delete process.env.VERA_MANIFEST_URL;
  process.env.VERA_RELEASE_BASE_URL = base;
  t.after(async () => {
    for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key];
    Object.assign(process.env, saved);
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await fsp.rm(temp, { recursive: true, force: true });
  });
  return { temp, base, server };
}

function manifest(base, archive, version = packageVersion) {
  return { version, assets: { [target]: {
    archive: `vera-${target}.tar.gz`, size: archive.length,
    sha256: crypto.createHash("sha256").update(archive).digest("hex"), download_url: `${base}/archive`,
  } } };
}

function serveManifest(base, archive, request, response, changes = {}) {
  if (request.url === "/archive") response.end(archive);
  else response.end(JSON.stringify(Object.assign(manifest(base, archive), changes)));
}

test("verified small cache runs offline, including an explicit manifest version", async (t) => {
  const archive = tar([{ name: member, contents: "#!/bin/sh\nprintf '%s\\n' \"$@\"\n" }]);
  let base;
  let requests = 0;
  const f = await fixture(t, (req, res) => { requests++; serveManifest(base, archive, req, res, { version: "9.8.7" }); });
  base = f.base;
  process.env.VERA_MANIFEST_URL = `${base}/custom`;
  const first = await wrapper.ensureBinaryInstalled();
  assert.equal(first.version, "9.8.7");
  assert.equal(requests, 2);
  await new Promise((resolve) => f.server.close(resolve));
  const second = await wrapper.ensureBinaryInstalled();
  assert.deepEqual(second, first);
  assert.equal(requests, 2);
  if (process.platform !== "win32") {
    const result = spawnSync(first.binaryPath, ["space here", "--flag"], { encoding: "utf8" });
    assert.equal(result.stdout, "space here\n--flag\n");
  }
});

test("default selection never falls back to latest or accepts another version", async (t) => {
  const seen = [];
  const f = await fixture(t, (req, res) => { seen.push(req.url); res.writeHead(404); res.end(); });
  await assert.rejects(wrapper.ensureBinaryInstalled(), /404/);
  assert.deepEqual(seen, [`/releases/download/v${packageVersion}/release-manifest.json`]);
  f.server.removeAllListeners("request");
  f.server.on("request", (req, res) => res.end(JSON.stringify({ version: "9.8.7" })));
  await assert.rejects(wrapper.ensureBinaryInstalled(), /requested Vera/);
});

test("corrupt or unfinished caches are repaired and failed downloads leave no binary", async (t) => {
  const archive = tar([{ name: member, contents: "complete binary" }]);
  let base;
  let broken = false;
  const f = await fixture(t, (req, res) => {
    if (req.url === "/archive" && broken) { res.writeHead(200, { "Content-Length": archive.length }); res.write(archive.subarray(0, 10)); res.destroy(); }
    else serveManifest(base, archive, req, res);
  });
  base = f.base;
  const legacy = path.join(process.env.VERA_HOME, "bin", packageVersion, target, binaryName);
  await fsp.mkdir(path.dirname(legacy), { recursive: true });
  await fsp.writeFile(legacy, Buffer.alloc(1_000_001));
  const installed = await wrapper.ensureBinaryInstalled();
  await fsp.writeFile(installed.binaryPath, "broken");
  const repaired = await wrapper.ensureBinaryInstalled();
  assert.equal(await fsp.readFile(repaired.binaryPath, "utf8"), "complete binary");
  await fsp.rm(repaired.binaryPath);
  broken = true;
  await assert.rejects(wrapper.ensureBinaryInstalled());
  assert.equal(fs.existsSync(repaired.binaryPath), false);
  assert.equal((await fsp.readdir(path.dirname(repaired.binaryPath))).some((name) => name.startsWith(".install-")), false);
});

test("bounded requests reject redirects, oversized bodies, stalls, sizes and checksums", async (t) => {
  const f = await fixture(t, (req, res) => {
    if (req.url === "/redirect") { res.writeHead(302, { Location: "/redirect" }); res.end(); }
    else if (req.url === "/unsafe") { res.writeHead(302, { Location: "file:///outside" }); res.end(); }
    else if (req.url === "/large") res.end(Buffer.alloc(1024 * 1024 + 1));
    else if (req.url === "/stall") { res.writeHead(200); res.flushHeaders(); }
    else res.end("short");
  });
  await assert.rejects(wrapper.fetchText(`${f.base}/redirect`), /redirect/);
  await assert.rejects(wrapper.fetchText(`${f.base}/unsafe`), /HTTP or HTTPS/);
  await assert.rejects(wrapper.fetchText(`${f.base}/large`), /size limit/);
  await assert.rejects(wrapper.fetchText(`${f.base}/stall`, 50));
  const dest = path.join(f.temp, "download");
  await assert.rejects(wrapper.downloadFile(`${f.base}/short`, dest, 10), /size mismatch/);
  assert.equal(fs.existsSync(dest), false);
  await assert.rejects(wrapper.downloadFile(`${f.base}/short`, dest, 2), /size limit/);
  const archive = tar([{ name: member, contents: "binary" }]);
  f.server.removeAllListeners("request");
  f.server.on("request", (req, res) => {
    const value = manifest(f.base, archive);
    value.assets[target].sha256 = "0".repeat(64);
    if (req.url === "/archive") res.end(archive); else res.end(JSON.stringify(value));
  });
  await assert.rejects(wrapper.ensureBinaryInstalled(), /checksum mismatch/);
});

test("tar accepts only the expected regular binary without writing archive paths", async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const archivePath = path.join(f.temp, "archive.tar.gz");
  const cases = [
    [{ name: member, contents: "binary" }, { name: "../outside", contents: "canary" }],
    [{ name: member, type: "2", contents: "" }],
    [{ name: member, type: "1", contents: "" }],
    [{ name: member, contents: "a" }, { name: member, contents: "b" }],
    [{ name: "/outside", contents: "canary" }, { name: member, contents: "binary" }],
    [{ name: "C:\\outside", contents: "canary" }],
    [{ name: member, contents: "" }],
  ];
  for (let index = 0; index < cases.length; index++) {
    await fsp.writeFile(archivePath, tar(cases[index]));
    await assert.rejects(wrapper.extractArchive(archivePath, path.join(f.temp, `output-${index}`), target));
  }
  assert.equal(fs.existsSync(path.join(path.dirname(f.temp), "outside")), false);
  const valid = tar([{ name: `vera-${target}/`, type: "5" }, { name: member, contents: "binary" }]);
  await fsp.writeFile(archivePath, valid);
  const dest = path.join(f.temp, "valid");
  await wrapper.extractArchive(archivePath, dest, target);
  assert.equal(await fsp.readFile(dest, "utf8"), "binary");
  await fsp.writeFile(archivePath, valid.subarray(0, valid.length - 8));
  await assert.rejects(wrapper.extractArchive(archivePath, path.join(f.temp, "truncated"), target));
});

test("missing ldd detects musl via the linker fallback", () => {
  const code = fs.readFileSync(path.join(__dirname, "../bin/vera.js"), "utf8");
  const sandbox = { module: { exports: {} }, process: { platform: "linux" }, require(name) {
    if (name === "node:child_process") return { spawnSync: () => ({ error: new Error("ENOENT") }) };
    if (name === "node:fs") return { readdirSync: () => ["ld-musl-x86_64.so.1"] };
    if (name === "../package.json") return { version: packageVersion };
    return require(name);
  } };
  vm.runInNewContext(code, sandbox);
  assert.equal(sandbox.module.exports.detectMusl(), true);
});

test("shim quotes shell metacharacters and forwards arguments", { skip: process.platform === "win32" }, async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const binary = path.join(f.temp, "space ' $VAR `literal` $(touch canary)");
  await fsp.writeFile(binary, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n", { mode: 0o755 });
  const shim = await wrapper.createShim(binary);
  const result = spawnSync(shim, ["a b", "$literal"], { encoding: "utf8" });
  assert.equal(result.status, 0);
  assert.equal(result.stdout, "a b\n$literal\n");
});

test("child exit status and termination signal reach the caller", { skip: process.platform === "win32" }, async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const binary = path.join(f.temp, "child");
  const modulePath = path.join(__dirname, "../bin/vera.js");
  const run = () => spawnSync(process.execPath, ["-e", "require(process.argv[1]).runBinary(process.argv[2], [])", modulePath, binary]);
  await fsp.writeFile(binary, "#!/bin/sh\nexit 7\n", { mode: 0o755 });
  assert.equal(run().status, 7);
  await fsp.writeFile(binary, "#!/bin/sh\nkill -TERM $$\n", { mode: 0o755 });
  assert.equal(run().signal, "SIGTERM");
  await fsp.writeFile(binary, "#!/bin/sh\nprintf ready\\n\nexec sleep 30\n", { mode: 0o755 });
  const child = spawn(process.execPath, ["-e", "require(process.argv[1]).runBinary(process.argv[2], [])", modulePath, binary], { stdio: ["ignore", "pipe", "pipe"] });
  await new Promise((resolve) => child.stdout.once("data", resolve));
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => resolve(signal)));
  child.kill("SIGTERM");
  assert.equal(await exited, "SIGTERM");
});

test("Windows ZIP extraction validates entries and writes only the expected binary", { skip: process.platform !== "win32" }, async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const windowsTarget = "x86_64-pc-windows-msvc";
  const expected = `vera-${windowsTarget}/vera.exe`;
  const archive = path.join(f.temp, "archive.zip");
  const makeZip = (entries) => {
    const script = "import json,sys,zipfile; a=zipfile.ZipFile(sys.argv[1],'w'); " +
      "[(lambda i: (setattr(i,'external_attr',e.get('mode',0)<<16), a.writestr(i,e.get('contents','binary'))))(zipfile.ZipInfo(e['name'])) for e in json.loads(sys.argv[2])]; a.close()";
    const result = spawnSync("python", ["-c", script, archive, JSON.stringify(entries)], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
  };
  const cases = [
    [{ name: expected }, { name: "../outside" }],
    [{ name: expected }, { name: expected }],
    [{ name: expected, mode: 0o120777 }],
    [{ name: expected, mode: 0o40755 }],
    [{ name: "C:\\outside" }, { name: expected }],
    [{ name: "/outside" }, { name: expected }],
  ];
  for (let index = 0; index < cases.length; index++) {
    makeZip(cases[index]);
    await assert.rejects(wrapper.extractArchive(archive, path.join(f.temp, `zip-${index}`), windowsTarget));
  }
  makeZip([{ name: expected, contents: "verified binary" }]);
  const output = path.join(f.temp, "valid.exe");
  await wrapper.extractArchive(archive, output, windowsTarget);
  assert.equal(await fsp.readFile(output, "utf8"), "verified binary");
  assert.equal(fs.existsSync(path.join(path.dirname(f.temp), "outside")), false);
});

test("Windows shim preserves spaces and literal percent and exclamation marks", { skip: process.platform !== "win32" }, async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const binary = path.join(f.temp, "space %USERPROFILE% ! literal.exe");
  await fsp.copyFile(process.execPath, binary);
  const shim = await wrapper.createShim(binary);
  const result = spawnSync(process.env.ComSpec || "cmd.exe", ["/d", "/s", "/c", `""${shim}" --version"`], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), process.version);
});


test("shim publication preserves its target and colocated native binary", { skip: process.platform === "win32" }, async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const binary = path.join(f.temp, "native");
  await fsp.writeFile(binary, "native binary", { mode: 0o755 });
  await fsp.mkdir(process.env.VERA_USER_BIN_DIR, { recursive: true });
  const shim = path.join(process.env.VERA_USER_BIN_DIR, "vera");
  await fsp.symlink(binary, shim);
  await wrapper.createShim(binary);
  assert.equal(await fsp.readFile(binary, "utf8"), "native binary");
  const colocated = path.join(process.env.VERA_USER_BIN_DIR, "vera");
  await fsp.rm(colocated);
  await fsp.writeFile(colocated, "colocated binary", { mode: 0o755 });
  await wrapper.createShim(colocated);
  assert.equal(await fsp.readFile(colocated, "utf8"), "colocated binary");
});
