import hashlib
import http.server
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest
from unittest.mock import patch
import zipfile

MODULE = Path(__file__).parents[1] / "src/vera_ai_wrapper/__main__.py"
spec = importlib.util.spec_from_file_location("wrapper", MODULE)
wrapper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wrapper)
TARGET = "x86_64-unknown-linux-gnu"
MEMBER = f"vera-{TARGET}/{wrapper.binary_name()}"
SHIM_CONTRACT = json.loads((Path(__file__).parents[2] / "shim-contract.json").read_text())


def tar(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, contents, kind in entries:
            info = tarfile.TarInfo(name)
            info.type = kind
            info.size = len(contents)
            archive.addfile(info, io.BytesIO(contents))
    return output.getvalue()


class Contracts(unittest.TestCase):
    def test_both_platform_helpers_match_every_shared_fixture(self):
        for platform, cases in SHIM_CONTRACT.items():
            for case in cases:
                with self.subTest(platform=platform, path=case["binary_path"]):
                    self.assertEqual(wrapper.shim_contents(case["binary_path"], platform == "windows"), case["shim"])
                    self.assertEqual(wrapper.shim_target(case["shim"]), case["binary_path"])

    def test_home_resolution_matches_rust(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {}, clear=True):
            home = Path(temp) / "user"
            legacy = home / ".vera"
            with patch.object(wrapper.Path, "home", return_value=home), patch.object(wrapper.platform, "system", return_value="Linux"):
                os.environ["XDG_DATA_HOME"] = str(Path(temp) / "xdg")
                self.assertEqual(wrapper.vera_home(), Path(temp) / "xdg/vera")
                os.environ["XDG_DATA_HOME"] = "relative"
                self.assertEqual(wrapper.vera_home(), home / ".local/share/vera")
                with patch.object(wrapper.platform, "system", return_value="Darwin"):
                    self.assertEqual(wrapper.vera_home(), home / "Library/Application Support/vera")
                with patch.object(wrapper.platform, "system", return_value="Windows"):
                    self.assertEqual(wrapper.vera_home(), legacy)
                    os.environ["APPDATA"] = str(Path(temp) / "roaming")
                    self.assertEqual(wrapper.vera_home(), Path(temp) / "roaming/vera")
                legacy.mkdir(parents=True)
                (legacy / "update-check.json").write_text("{}")
                (legacy / ".hidden").write_text("incidental")
                self.assertEqual(wrapper.vera_home(), home / ".local/share/vera")
                (legacy / "models").mkdir()
                self.assertEqual(wrapper.vera_home(), legacy)
                os.environ["VERA_HOME"] = " \t "
                self.assertEqual(wrapper.vera_home(), legacy)
                os.environ["VERA_HOME"] = "~/raw home "
                self.assertEqual(wrapper.vera_home(), Path("~/raw home "))
                os.environ.pop("VERA_HOME")
                (legacy / "models").rmdir()
                (legacy / "update-check.json").unlink()
                (legacy / ".hidden").unlink()
                legacy.rmdir()
                legacy.write_text("not a directory")
                with self.assertRaises(NotADirectoryError):
                    wrapper.vera_home()

    def test_agent_prompts_require_both_terminals_unless_args_are_explicit(self):
        for stdin, stderr, rest in [(True, True, []), (True, False, []), (False, True, []), (False, False, []), (False, False, ["--client", "all", "--scope", "global"])]:
            with self.subTest(stdin=stdin, stderr=stderr, rest=rest), patch.object(sys, "argv", ["vera-ai", "install", *rest]), \
                    patch.object(wrapper, "ensure_binary_installed", return_value=(Path("binary"), "1.0.0")) as ensure, \
                    patch.object(wrapper, "run_binary", return_value=7) as run, \
                    patch.object(sys.stdin, "isatty", return_value=stdin), \
                    patch.object(sys, "stderr", new_callable=io.StringIO) as output:
                with patch.object(output, "isatty", return_value=stderr):
                    expected = bool(rest) or (stdin and stderr)
                    self.assertEqual(wrapper.run(), 7 if expected else 0)
                ensure.assert_called_once_with(install=True)
                if expected:
                    run.assert_called_once_with(Path("binary"), ["agent", "install", *rest])
                else:
                    run.assert_not_called()
                    self.assertIn("vera agent install --client all --scope global", output.getvalue())
                self.assertIn("Vera 1.0.0 installed.", output.getvalue())


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="vera-wrapper-test-")
        self.root = Path(self.temp.name)
        self.seen = []
        self.respond = lambda handler: handler.send_error(404)
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                owner.seen.append(self.path)
                owner.respond(self)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base = f"http://127.0.0.1:{self.server.server_port}"
        self.env = patch.dict(os.environ, {
            "VERA_HOME": str(self.root / "home"), "VERA_USER_BIN_DIR": str(self.root / "bin"),
            "VERA_TARGET": TARGET, "VERA_RELEASE_BASE_URL": self.base,
        })
        self.env.start()
        os.environ.pop("VERA_MANIFEST_URL", None)
        self.version = patch.object(wrapper, "package_version", return_value="1.4.0")
        self.version.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        self.version.stop()
        self.env.stop()
        self.temp.cleanup()

    def archive(self, content=b"complete binary"):
        return tar([(MEMBER, content, tarfile.REGTYPE)])

    def manifest(self, archive, version="1.4.0"):
        return {"version": version, "assets": {TARGET: {
            "archive": f"vera-{TARGET}.tar.gz", "size": len(archive),
            "sha256": hashlib.sha256(archive).hexdigest(), "download_url": f"{self.base}/archive",
        }}}

    def serve(self, archive, version="1.4.0", checksum=None):
        value = self.manifest(archive, version)
        if checksum:
            value["assets"][TARGET]["sha256"] = checksum

        def respond(handler):
            handler.send_response(200)
            handler.end_headers()
            handler.wfile.write(archive if handler.path == "/archive" else json.dumps(value).encode())

        self.respond = respond

    def test_verified_cache_works_offline_and_override_version_is_preserved(self):
        self.serve(self.archive(), "9.8.7")
        os.environ["VERA_MANIFEST_URL"] = f"{self.base}/custom"
        first = wrapper.ensure_binary_installed()
        self.assertEqual(first[1], "9.8.7")
        self.assertEqual(len(self.seen), 2)
        with patch.object(wrapper, "open_response", side_effect=OSError("offline")):
            self.assertEqual(wrapper.ensure_binary_installed(), first)
        self.assertEqual(len(self.seen), 2)

    def test_requested_version_never_falls_back_to_latest(self):
        with self.assertRaisesRegex(RuntimeError, "HTTP 404"):
            wrapper.ensure_binary_installed()
        self.assertEqual(self.seen, ["/releases/download/v1.4.0/release-manifest.json"])
        self.serve(self.archive(), "9.8.7")
        with self.assertRaisesRegex(RuntimeError, "requested Vera"):
            wrapper.ensure_binary_installed()

    def test_prerelease_download_retains_exact_tag_after_pypi_normalization(self):
        self.version.stop()
        (self.root / "release-version.txt").write_text("1.4.0-rc.1\n")
        with patch.object(wrapper, "__file__", str(self.root / "__main__.py")), \
                patch.object(wrapper, "version", return_value="1.4.0rc1"):
            self.assertEqual(wrapper.package_version(), "1.4.0-rc.1")
            self.serve(self.archive(), "1.4.0-rc.1")
            binary, value = wrapper.ensure_binary_installed()
            self.assertEqual(value, "1.4.0-rc.1")
            self.assertEqual(binary.read_bytes(), b"complete binary")
            self.assertEqual(self.seen[0], "/releases/download/v1.4.0-rc.1/release-manifest.json")
        self.version.start()

    def test_corrupt_and_incomplete_caches_are_repaired(self):
        self.serve(self.archive())
        legacy = wrapper.vera_home() / "bin/1.4.0" / TARGET / wrapper.binary_name()
        legacy.parent.mkdir(parents=True)
        legacy.write_bytes(b"0" * 1_000_001)
        binary, _ = wrapper.ensure_binary_installed()
        binary.write_bytes(b"broken")
        wrapper.ensure_binary_installed()
        self.assertEqual(binary.read_bytes(), b"complete binary")
        binary.unlink()

        def interrupted(handler):
            if handler.path != "/archive":
                handler.send_response(200)
                handler.end_headers()
                handler.wfile.write(json.dumps(self.manifest(self.archive())).encode())
                return
            handler.send_response(200)
            handler.send_header("Content-Length", "100")
            handler.end_headers()
            handler.wfile.write(b"short")
            handler.connection.shutdown(socket.SHUT_RDWR)
            handler.connection.close()

        self.respond = interrupted
        with self.assertRaises(Exception):
            wrapper.ensure_binary_installed()
        self.assertFalse(binary.exists())
        self.assertFalse(any(p.name.startswith(".install-") for p in binary.parent.iterdir()))

    def test_passthrough_preserves_launcher_and_metadata_install_or_missing_launcher_publishes(self):
        self.serve(self.archive())
        binary, value = wrapper.ensure_binary_installed()
        shim = wrapper.pick_user_bin_dir() / wrapper.shim_name()
        metadata = wrapper.install_metadata_path()
        original = shim.read_bytes()
        metadata.write_text('{"install_method":"manual","version":"old"}\n')
        os.utime(shim, (1, 1))
        os.utime(metadata, (1, 1))
        wrapper.ensure_binary_installed()
        self.assertEqual(shim.stat().st_mtime_ns, 1_000_000_000)
        self.assertEqual(metadata.stat().st_mtime_ns, 1_000_000_000)
        wrapper.ensure_binary_installed(install=True)
        self.assertEqual(shim.stat().st_mtime_ns, 1_000_000_000)
        self.assertEqual(json.loads(metadata.read_text())["version"], value)
        shim.unlink()
        metadata.write_text("{}")
        wrapper.ensure_binary_installed()
        self.assertEqual(shim.read_bytes(), original)
        self.assertEqual(json.loads(metadata.read_text())["binary_path"], str(binary))

    def test_shim_replacement_accepts_only_owned_templates_and_preserves_foreign_entries(self):
        binary = wrapper.vera_home() / "bin/2.0.1/x" / wrapper.binary_name()
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"native binary")
        shim = wrapper.create_shim(binary)
        old = wrapper.vera_home() / "bin/old" / wrapper.binary_name()
        bodies = [
            wrapper.shim_contents(str(old), False), wrapper.shim_contents(str(old), True),
            f'@echo off\r\n"{old}" %*\r\n',
            wrapper.shim_contents(str(Path.home() / ".vera/bin/old" / wrapper.binary_name()), os.name == "nt"),
            wrapper.shim_contents(str(wrapper.vera_home() / "bin/version/../old" / wrapper.binary_name()), os.name == "nt"),
        ]
        if not any(ch in str(old) for ch in "$`\\"):
            bodies.append(f'#!/bin/sh\nexec "{old}" "$@"\n')
        if wrapper.re.fullmatch(r"[A-Za-z0-9@%+=:,./_-]+", str(old)):
            bodies.append(f'#!/bin/sh\nexec {old} "$@"\n')
        for body in bodies:
            with self.subTest(body=body):
                shim.write_bytes(body.encode())
                self.assertEqual(wrapper.create_shim(binary), shim)
                self.assertEqual(shim.read_bytes().decode(), wrapper.shim_contents(str(binary), os.name == "nt"))
        foreign = [
            b"#!/bin/sh\necho other\n", b"\x7f\xcf",
            wrapper.shim_contents(str(wrapper.vera_home() / "bin-extra/vera"), os.name == "nt").encode(),
            (wrapper.shim_contents(str(old), False) + "echo extra\n").encode(),
            *[f'#!/bin/sh\nexec "{wrapper.vera_home() / "bin" / name}" "$@"\n'.encode()
              for name in ["$OTHER", "`other`", "back\\slash"]],
            wrapper.shim_contents(str(wrapper.vera_home() / "bin/../../other/tool"), os.name == "nt").encode(),
        ]
        os.environ["VERA_HOME"] = os.path.relpath(wrapper.vera_home())
        foreign.append(wrapper.shim_contents(str(wrapper.vera_home() / "bin/old" / wrapper.binary_name()), os.name == "nt").encode())
        with patch.object(sys, "stderr", new_callable=io.StringIO) as output:
            for body in foreign:
                shim.write_bytes(body)
                self.assertIsNone(wrapper.create_shim(binary))
                self.assertEqual(shim.read_bytes(), body)
            shim.unlink()
            shim.mkdir()
            self.assertIsNone(wrapper.create_shim(binary))
            shim.rmdir()
            if os.name != "nt":
                for target in [binary, self.root / "missing"]:
                    shim.symlink_to(target)
                    self.assertIsNone(wrapper.create_shim(binary))
                    self.assertTrue(shim.is_symlink())
                    shim.unlink()
            lines = output.getvalue().splitlines()
            self.assertEqual(len(lines), len(foreign) + 1 + (2 if os.name != "nt" else 0))
            self.assertTrue(all(str(shim) in line and str(binary) in line for line in lines))

    def test_blocked_shim_does_not_announce_path_addition(self):
        self.serve(self.archive())
        bin_dir = wrapper.pick_user_bin_dir()
        bin_dir.mkdir(parents=True)
        shim = bin_dir / wrapper.shim_name()
        shim.write_text("foreign")
        with patch.object(sys, "stderr", new_callable=io.StringIO) as output:
            binary, _ = wrapper.ensure_binary_installed(install=True)
        self.assertEqual(shim.read_text(), "foreign")
        self.assertIn(str(shim), output.getvalue())
        self.assertIn(str(binary), output.getvalue())
        self.assertNotIn("Added Vera", output.getvalue())

    def test_manifest_override_with_existing_launcher_records_download_and_then_runs_offline(self):
        self.serve(self.archive(), "9.8.7")
        os.environ["VERA_MANIFEST_URL"] = f"{self.base}/custom"
        bin_dir = wrapper.pick_user_bin_dir()
        bin_dir.mkdir(parents=True)
        shim = bin_dir / wrapper.shim_name()
        shim.write_text("existing launcher")
        os.utime(shim, (1, 1))
        first = wrapper.ensure_binary_installed()
        self.assertEqual(first[1], "9.8.7")
        self.assertEqual(len(self.seen), 2)
        self.assertEqual(shim.read_text(), "existing launcher")
        self.assertEqual(shim.stat().st_mtime_ns, 1_000_000_000)
        metadata = wrapper.install_metadata_path()
        self.assertEqual(json.loads(metadata.read_text())["binary_path"], str(first[0]))
        os.utime(metadata, (1, 1))
        with patch.object(wrapper, "open_response", side_effect=OSError("offline")):
            self.assertEqual(wrapper.ensure_binary_installed(), first)
        self.assertEqual(len(self.seen), 2)
        self.assertEqual(shim.stat().st_mtime_ns, 1_000_000_000)
        self.assertEqual(metadata.stat().st_mtime_ns, 1_000_000_000)

    def test_checksums_sizes_redirects_manifest_limit_and_timeouts(self):
        self.serve(self.archive(), checksum="0" * 64)
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            wrapper.ensure_binary_installed()

        def respond(handler):
            if handler.path == "/redirect":
                handler.send_response(302)
                handler.send_header("Location", "/redirect")
                handler.end_headers()
            elif handler.path == "/unsafe":
                handler.send_response(302)
                handler.send_header("Location", "file:///outside")
                handler.end_headers()
            else:
                handler.send_response(200)
                handler.end_headers()
                if handler.path == "/large":
                    handler.wfile.write(b" " * (wrapper.MAX_MANIFEST_BYTES + 1))
                elif handler.path == "/stall":
                    threading.Event().wait(0.2)
                else:
                    handler.wfile.write(b"short")

        self.respond = respond
        with self.assertRaisesRegex(RuntimeError, "HTTP 302"):
            wrapper.read_json(f"{self.base}/redirect")
        with self.assertRaises(RuntimeError):
            wrapper.read_json(f"{self.base}/unsafe")
        with self.assertRaisesRegex(RuntimeError, "size limit"):
            wrapper.read_json(f"{self.base}/large")
        with patch.object(wrapper, "REQUEST_TIMEOUT", 0.05), self.assertRaises(Exception):
            wrapper.read_json(f"{self.base}/stall")
        destination = self.root / "download"
        with self.assertRaisesRegex(RuntimeError, "size mismatch"):
            wrapper.download_file(f"{self.base}/short", destination, 10)
        self.assertFalse(destination.exists())
        with self.assertRaisesRegex(RuntimeError, "size limit"):
            wrapper.download_file(f"{self.base}/short", destination, 2)
        self.assertFalse(destination.exists())

    def test_tar_member_paths_links_duplicates_and_empty_binary_are_rejected(self):
        cases = [
            [(MEMBER, b"binary", tarfile.REGTYPE), ("../outside", b"canary", tarfile.REGTYPE)],
            [(MEMBER, b"", tarfile.SYMTYPE)], [(MEMBER, b"", tarfile.LNKTYPE)],
            [(MEMBER, b"a", tarfile.REGTYPE), (MEMBER, b"b", tarfile.REGTYPE)],
            [("/outside", b"canary", tarfile.REGTYPE)],
            [("C:\\outside", b"canary", tarfile.REGTYPE)],
            [(MEMBER, b"", tarfile.REGTYPE)],
        ]
        archive = self.root / "archive.tar.gz"
        for index, entries in enumerate(cases):
            archive.write_bytes(tar(entries))
            with self.subTest(entries=entries), self.assertRaises(RuntimeError):
                wrapper.extract_archive(archive, self.root / f"output-{index}", TARGET)
        self.assertFalse((self.root.parent / "outside").exists())
        archive.write_bytes(tar([(f"vera-{TARGET}/", b"", tarfile.DIRTYPE), (MEMBER, b"binary", tarfile.REGTYPE)]))
        destination = self.root / "valid"
        wrapper.extract_archive(archive, destination, TARGET)
        self.assertEqual(destination.read_bytes(), b"binary")

    def test_zip_member_paths_links_duplicates_and_bad_crc_are_rejected(self):
        archive = self.root / "archive.zip"
        for index, names in enumerate([[MEMBER, "../outside"], [MEMBER, MEMBER], ["/outside"], ["C:\\outside"]]):
            with zipfile.ZipFile(archive, "w") as handle:
                for name in names:
                    handle.writestr(name, b"binary")
            with self.subTest(names=names), self.assertRaises(RuntimeError):
                wrapper.extract_archive(archive, self.root / f"zip-{index}", TARGET)
        with zipfile.ZipFile(archive, "w") as handle:
            info = zipfile.ZipInfo(MEMBER)
            info.external_attr = (stat.S_IFLNK | 0o777) << 16
            handle.writestr(info, b"outside")
        with self.assertRaisesRegex(RuntimeError, "unsafe"):
            wrapper.extract_archive(archive, self.root / "zip-link", TARGET)
        with zipfile.ZipFile(archive, "w") as handle:
            handle.writestr(MEMBER, b"binary")
        wrapper.extract_archive(archive, self.root / "zip-valid", TARGET)
        self.assertEqual((self.root / "zip-valid").read_bytes(), b"binary")
        data = archive.read_bytes().replace(b"binary", b"broken", 1)
        archive.write_bytes(data)
        with self.assertRaises(zipfile.BadZipFile):
            wrapper.extract_archive(archive, self.root / "zip-crc", TARGET)

    def test_missing_ldd_musl_fallback(self):
        with patch.object(wrapper.platform, "system", return_value="Linux"), patch.object(
            wrapper.subprocess, "run", side_effect=FileNotFoundError
        ), patch.object(wrapper.os, "listdir", return_value=["ld-musl-x86_64.so.1"]):
            self.assertTrue(wrapper._detect_musl())

    @unittest.skipIf(os.name == "nt", "Unix shell and signal behavior")
    def test_shim_quoting_args_status_and_signals(self):
        binary = self.root / "space ' $VAR `literal` $(touch canary)"
        binary.write_text('#!/bin/sh\nprintf \'%s\\n\' "$@"\n')
        binary.chmod(0o755)
        shim = wrapper.create_shim(binary)
        result = subprocess.run([str(shim), "a b", "$literal"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "a b\n$literal\n")
        command = [sys.executable, "-c", "import runpy,sys; m=runpy.run_path(sys.argv[1]); m['run_binary'](m['Path'](sys.argv[2]), [])", str(MODULE), str(binary)]
        binary.write_text("#!/bin/sh\nexit 7\n")
        self.assertEqual(subprocess.run(command).returncode, 7)
        binary.write_text("#!/bin/sh\nkill -TERM $$\n")
        self.assertEqual(subprocess.run(command).returncode, -15)
        binary.write_text("#!/bin/sh\nprintf ready\\n\nexec sleep 30\n")
        with subprocess.Popen(command, stdout=subprocess.PIPE) as process:
            self.assertEqual(process.stdout.read(5), b"ready")
            process.terminate()
            self.assertEqual(process.wait(timeout=5), -15)


    @unittest.skipIf(os.name == "nt", "Unix symlink")
    def test_shim_preserves_symlink_target_and_colocated_binary(self):
        binary = self.root / "native"
        binary.write_bytes(b"native binary")
        bin_dir = wrapper.pick_user_bin_dir()
        bin_dir.mkdir(parents=True, exist_ok=True)
        shim = bin_dir / wrapper.shim_name()
        shim.symlink_to(binary)
        wrapper.create_shim(binary)
        self.assertEqual(binary.read_bytes(), b"native binary")
        shim.write_bytes(b"colocated binary")
        wrapper.create_shim(shim)
        self.assertEqual(shim.read_bytes(), b"colocated binary")

    @unittest.skipUnless(os.name == "nt", "Windows batch shim")
    def test_windows_shim_quotes_literal_paths(self):
        import shutil
        binary = self.root / "space %USERPROFILE% ! literal.exe"
        shutil.copyfile(os.environ.get("COMSPEC", "C:/Windows/System32/cmd.exe"), binary)
        shim = wrapper.create_shim(binary)
        command = os.environ.get("COMSPEC", "cmd.exe")
        result = subprocess.run(
            f'"{command}" /d /s /c ""{shim}" /d /c echo shim-works"',
            executable=command, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "shim-works")


if __name__ == "__main__":
    unittest.main()
