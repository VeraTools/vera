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
const shimContract = require("../../shim-contract.json");
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

function sandboxWrapper(overrides = {}) {
  const sandbox = { module: { exports: {} }, process, require, ...overrides };
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(path.join(__dirname, "../bin/vera.js"), "utf8"), sandbox);
  return sandbox;
}

test("both platform helpers match every shared shim fixture", () => {
  for (const [platform, cases] of Object.entries(shimContract)) {
    for (const value of cases) {
      assert.equal(wrapper.shimContents(value.binary_path, platform === "windows" ? "win32" : "linux"), value.shim);
    }
  }
});

test("npm exposes vera-ai without a postinstall hook", () => {
  const pkg = require("../package.json");
  assert.deepEqual(pkg.bin, { "vera-ai": "./bin/vera.js" });
  assert.equal(pkg.scripts?.postinstall, undefined);
});

test("home resolution follows Rust overrides, legacy detection and platform data dirs", async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const home = path.join(f.temp, "user");
  const legacy = path.join(home, ".vera");
  const env = { XDG_DATA_HOME: path.join(f.temp, "xdg"), APPDATA: path.join(f.temp, "roaming") };
  const resolve = (platform) => sandboxWrapper({ process: { platform, env }, require(name) {
    if (name === "node:os") return { homedir: () => home };
    if (name === "../package.json") return { version: packageVersion };
    return require(name);
  } }).module.exports.defaultVeraHome();
  assert.equal(resolve("linux"), path.join(env.XDG_DATA_HOME, "vera"));
  env.XDG_DATA_HOME = "relative";
  assert.equal(resolve("linux"), path.join(home, ".local", "share", "vera"));
  assert.equal(resolve("darwin"), path.join(home, "Library", "Application Support", "vera"));
  assert.equal(resolve("win32"), path.join(env.APPDATA, "vera"));
  delete env.APPDATA;
  assert.equal(resolve("win32"), legacy);
  await fsp.mkdir(legacy, { recursive: true });
  await fsp.writeFile(path.join(legacy, "update-check.json"), "{}");
  await fsp.writeFile(path.join(legacy, ".hidden"), "incidental");
  assert.equal(resolve("linux"), path.join(home, ".local", "share", "vera"));
  await fsp.mkdir(path.join(legacy, "models"));
  assert.equal(resolve("linux"), legacy);
  env.VERA_HOME = " \t ";
  assert.equal(resolve("linux"), legacy);
  env.VERA_HOME = "~/raw home ";
  assert.equal(resolve("linux"), path.resolve("~/raw home "));
  await fsp.rm(legacy, { recursive: true });
  await fsp.writeFile(legacy, "not a directory");
  delete env.VERA_HOME;
  assert.throws(() => resolve("linux"), /ENOTDIR/);
});

test("passthrough preserves launchers and provenance; install or a missing launcher publishes them", async (t) => {
  const archive = tar([{ name: member, contents: "binary" }]);
  let base;
  const f = await fixture(t, (req, res) => serveManifest(base, archive, req, res));
  base = f.base;
  const installed = await wrapper.ensureBinaryInstalled();
  const shim = path.join(process.env.VERA_USER_BIN_DIR, process.platform === "win32" ? "vera.cmd" : "vera");
  const metadata = path.join(process.env.VERA_HOME, "install.json");
  const originalShim = await fsp.readFile(shim);
  await fsp.writeFile(metadata, '{"install_method":"manual","version":"old"}\n');
  const stamp = new Date(1000);
  await fsp.utimes(shim, stamp, stamp);
  await fsp.utimes(metadata, stamp, stamp);
  await wrapper.ensureBinaryInstalled();
  assert.equal((await fsp.stat(shim)).mtimeMs, 1000);
  assert.equal((await fsp.stat(metadata)).mtimeMs, 1000);
  await wrapper.ensureBinaryInstalled(true);
  assert.equal((await fsp.stat(shim)).mtimeMs, 1000, "identical shim is not rewritten");
  assert.equal(JSON.parse(await fsp.readFile(metadata)).version, installed.version);
  await fsp.rm(shim);
  await fsp.writeFile(metadata, "{}");
  await wrapper.ensureBinaryInstalled();
  assert.deepEqual(await fsp.readFile(shim), originalShim);
  assert.equal(JSON.parse(await fsp.readFile(metadata)).binary_path, installed.binaryPath);
});

test("headless install skips prompts but explicit arguments still reach agent install", { skip: process.platform === "win32" }, async (t) => {
  const archive = tar([{ name: member, contents: "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 7\n" }]);
  let base;
  const f = await fixture(t, (req, res) => serveManifest(base, archive, req, res));
  base = f.base;
  await wrapper.ensureBinaryInstalled();
  const run = (...args) => spawnSync(process.execPath, [path.join(__dirname, "../bin/vera.js"), "install", ...args], { encoding: "utf8" });
  const bare = run();
  assert.equal(bare.status, 0, bare.stderr);
  assert.equal(bare.stdout, "");
  assert.ok(bare.stderr.includes(`Vera ${packageVersion} installed.`));
  assert.ok(bare.stderr.includes("vera agent install --client all --scope global"));
  const explicit = run("--client", "all", "--scope", "global");
  assert.equal(explicit.status, 7);
  assert.equal(explicit.stdout, "agent\ninstall\n--client\nall\n--scope\nglobal\n");
});

test("agent prompts require both stdin and stderr terminals", async () => {
  for (const [stdin, stderr] of [[true, true], [true, false], [false, true], [false, false]]) {
    const messages = [];
    const sandbox = sandboxWrapper({ process: { argv: ["node", "vera.js", "install"], stdin: { isTTY: stdin }, stderr: { isTTY: stderr } }, console: { error: (line) => messages.push(line) } });
    await vm.runInContext('ensureBinaryInstalled = async () => ({ binaryPath: "/binary", version: "1.0.0" }); runBinary = async () => { called = true; }; main();', sandbox);
    assert.equal(!!sandbox.called, stdin && stderr);
    assert.equal(messages.length, stdin && stderr ? 1 : 2);
  }
});

test("shim replacement accepts only owned templates and preserves foreign entries", async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const binary = path.join(process.env.VERA_HOME, "bin", "2.0.1", "x", binaryName);
  await fsp.mkdir(path.dirname(binary), { recursive: true });
  await fsp.writeFile(binary, "native binary");
  const shim = await wrapper.createShim(binary);
  const old = path.join(process.env.VERA_HOME, "bin", "2.0.0", "x", binaryName);
  const bodies = [
    wrapper.shimContents(old, "linux"), wrapper.shimContents(old, "win32"),
    `@echo off\r\n"${old}" %*\r\n`,
    wrapper.shimContents(path.join(os.homedir(), ".vera", "bin", "2.0.0", "x", binaryName), process.platform),
    wrapper.shimContents(path.join(process.env.VERA_HOME, "bin") + `${path.sep}version${path.sep}..${path.sep}2.0.0${path.sep}x${path.sep}${binaryName}`, process.platform),
  ];
  if (!/[$`\\"]/.test(old)) bodies.push(`#!/bin/sh\nexec "${old}" "$@"\n`);
  if (/^[A-Za-z0-9@%+=:,./_-]+$/.test(old)) bodies.push(`#!/bin/sh\nexec ${old} "$@"\n`);
  for (const body of bodies) {
    await fsp.writeFile(shim, body);
    assert.equal(await wrapper.createShim(binary), shim);
    assert.equal(await fsp.readFile(shim, "utf8"), wrapper.shimContents(binary, process.platform));
  }
  const foreign = [
    "#!/bin/sh\necho other\n", Buffer.from([0x7f, 0xcf]),
    wrapper.shimContents(path.join(process.env.VERA_HOME, "bin-extra", "vera"), process.platform),
    wrapper.shimContents(old, "linux") + "echo extra\n",
    ...["$OTHER", "`other`", "back\\slash", 'quo"te'].map((name) => `#!/bin/sh\nexec "${path.join(process.env.VERA_HOME, "bin", name)}" "$@"\n`),
    wrapper.shimContents(path.join(process.env.VERA_HOME, "bin") + `${path.sep}..${path.sep}..${path.sep}other${path.sep}tool`, process.platform),
    wrapper.shimContents(path.join(process.env.VERA_HOME, "bin", "other-tool"), process.platform),
    wrapper.shimContents(path.join("relative", "bin", "2.0.0", "x", binaryName), process.platform),
  ];
  const errors = [];
  const originalError = console.error;
  console.error = (line) => errors.push(line);
  t.after(() => { console.error = originalError; });
  for (const body of foreign) {
    await fsp.writeFile(shim, body);
    assert.equal(await wrapper.createShim(binary), null);
    assert.deepEqual(await fsp.readFile(shim), Buffer.from(body));
  }
  await fsp.rm(shim);
  await fsp.mkdir(shim);
  assert.equal(await wrapper.createShim(binary), null);
  await fsp.rm(shim, { recursive: true });
  if (process.platform !== "win32") {
    await fsp.symlink(binary, shim);
    assert.equal(await wrapper.createShim(binary), null);
    assert.equal((await fsp.lstat(shim)).isSymbolicLink(), true);
    await fsp.rm(shim);
    await fsp.symlink(path.join(f.temp, "missing"), shim);
    assert.equal(await wrapper.createShim(binary), null);
    assert.equal((await fsp.lstat(shim)).isSymbolicLink(), true);
  }
  for (const line of errors) assert.ok(line.includes(shim) && line.includes(binary));
  assert.equal(errors.length, foreign.length + 1 + (process.platform !== "win32" ? 2 : 0));
});

test("blocked shim does not announce a PATH addition", async (t) => {
  const archive = tar([{ name: member, contents: "binary" }]);
  let base;
  const f = await fixture(t, (req, res) => serveManifest(base, archive, req, res));
  base = f.base;
  await fsp.mkdir(process.env.VERA_USER_BIN_DIR, { recursive: true });
  const shim = path.join(process.env.VERA_USER_BIN_DIR, process.platform === "win32" ? "vera.cmd" : "vera");
  await fsp.writeFile(shim, "foreign");
  const messages = [];
  const originalError = console.error;
  console.error = (line) => messages.push(line);
  t.after(() => { console.error = originalError; });
  const installed = await wrapper.ensureBinaryInstalled(true);
  assert.equal(await fsp.readFile(shim, "utf8"), "foreign");
  assert.ok(messages.some((line) => line.includes(shim) && line.includes(installed.binaryPath)));
  assert.equal(messages.some((line) => line.startsWith("Added Vera")), false);
});

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

test("a manifest override with an existing launcher records a fresh download and then runs offline", async (t) => {
  const archive = tar([{ name: member, contents: "verified binary" }]);
  let base;
  let requests = 0;
  const f = await fixture(t, (req, res) => { requests++; serveManifest(base, archive, req, res, { version: "9.8.7" }); });
  base = f.base;
  process.env.VERA_MANIFEST_URL = `${base}/custom`;
  await fsp.mkdir(process.env.VERA_USER_BIN_DIR, { recursive: true });
  const shim = path.join(process.env.VERA_USER_BIN_DIR, process.platform === "win32" ? "vera.cmd" : "vera");
  await fsp.writeFile(shim, "existing launcher");
  await fsp.utimes(shim, new Date(1000), new Date(1000));
  const first = await wrapper.ensureBinaryInstalled();
  assert.equal(first.version, "9.8.7");
  assert.equal(requests, 2);
  assert.equal(await fsp.readFile(shim, "utf8"), "existing launcher");
  assert.equal((await fsp.stat(shim)).mtimeMs, 1000);
  const metadata = path.join(process.env.VERA_HOME, "install.json");
  assert.equal(JSON.parse(await fsp.readFile(metadata)).binary_path, first.binaryPath);
  await fsp.utimes(metadata, new Date(1000), new Date(1000));
  await new Promise((resolve) => f.server.close(resolve));
  assert.deepEqual(await wrapper.ensureBinaryInstalled(), first);
  assert.equal(requests, 2);
  assert.equal((await fsp.stat(shim)).mtimeMs, 1000);
  assert.equal((await fsp.stat(metadata)).mtimeMs, 1000);
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
  const sandbox = sandboxWrapper({ process: { platform: "linux" }, require(name) {
    if (name === "node:child_process") return { spawnSync: () => ({ error: new Error("ENOENT") }) };
    if (name === "node:fs") return { readdirSync: () => ["ld-musl-x86_64.so.1"] };
    if (name === "../package.json") return { version: packageVersion };
    return require(name);
  } });
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
  makeZip([{ name: expected, contents: "expands beyond declared size" }]);
  const forged = await fsp.readFile(archive);
  const directory = forged.indexOf(Buffer.from("504b0102", "hex"));
  assert.ok(directory >= 0);
  forged.writeUInt32LE(1, directory + 24);
  await fsp.writeFile(archive, forged);
  const bounded = path.join(f.temp, "bounded.exe");
  await assert.rejects(wrapper.extractArchive(archive, bounded, windowsTarget));
  assert.ok(!fs.existsSync(bounded) || (await fsp.stat(bounded)).size === 0);
  assert.equal(fs.existsSync(path.join(path.dirname(f.temp), "outside")), false);
});

test("Windows shim preserves spaces and literal percent and exclamation marks", { skip: process.platform !== "win32" }, async (t) => {
  const f = await fixture(t, (req, res) => res.end());
  const binary = path.join(f.temp, "space %USERPROFILE% ! literal.exe");
  await fsp.copyFile(process.execPath, binary);
  const shim = await wrapper.createShim(binary);
  const result = spawnSync(process.env.ComSpec || "cmd.exe", ["/d", "/s", "/c", `""${shim}" --version"`], { encoding: "utf8", windowsVerbatimArguments: true });
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
