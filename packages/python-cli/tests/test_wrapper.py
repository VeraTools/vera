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


def tar(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, contents, kind in entries:
            info = tarfile.TarInfo(name)
            info.type = kind
            info.size = len(contents)
            archive.addfile(info, io.BytesIO(contents))
    return output.getvalue()


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
        with self.assertRaises(Exception):
            wrapper.ensure_binary_installed()
        self.assertEqual(self.seen, ["/releases/download/v1.4.0/release-manifest.json"])
        self.serve(self.archive(), "9.8.7")
        with self.assertRaisesRegex(RuntimeError, "requested Vera"):
            wrapper.ensure_binary_installed()

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
        with self.assertRaises(Exception):
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
            with self.subTest(entries=entries), self.assertRaises(Exception):
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
        result = subprocess.run(
            [os.environ.get("COMSPEC", "cmd.exe"), "/d", "/s", "/c", f'""{shim}" /d /c echo shim-works"'],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "shim-works")


if __name__ == "__main__":
    unittest.main()
