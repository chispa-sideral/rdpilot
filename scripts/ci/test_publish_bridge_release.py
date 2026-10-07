"""Exercise the actual publisher against stateful, failing GitHub commands."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


def load_publisher():
    path = Path(__file__).with_name("publish-bridge-release.py")
    spec = importlib.util.spec_from_file_location("publish_bridge_release", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


publisher = load_publisher()
REAL_RUN = subprocess.run


class GitHub:
    def __init__(self, draft=None):
        self.release = None if draft is None else {"tag_name": "v0.2.0", "draft": draft, "id": 1}
        self.assets = {}
        self.history = []
        self.reads = 0
        self.change_at_read = {}
        self.lookup = None
        self.fail_upload = None
        self.fail_download = False
        self.corrupt = None
        self.missing = None

    def __call__(self, argv, **kwargs):
        if argv[0] == "git":
            return REAL_RUN(argv, **kwargs)
        self.history.append(argv)
        if argv[:2] == ["gh", "api"]:
            self.reads += 1
            if self.reads in self.change_at_read:
                self.release = self.change_at_read[self.reads]
            if self.lookup == "transport":
                return subprocess.CompletedProcess(argv, 1, "", "private credential text")
            if self.lookup == "malformed":
                return subprocess.CompletedProcess(argv, 0, "HTTP/2.0 200 OK\n\ninvalid JSON", "")
            if isinstance(self.lookup, int):
                return subprocess.CompletedProcess(argv, 1, f"HTTP/2.0 {self.lookup} Error\n\n{{}}", "")
            code = 404 if self.release is None else 200
            body = {} if self.release is None else self.release
            return subprocess.CompletedProcess(argv, 1 if code == 404 else 0,
                                               f"HTTP/2.0 {code} Response\r\nContent-Type: application/json\r\n\r\n"
                                               + json.dumps(body), "")
        operation = argv[2]
        if operation == "create":
            if self.release is not None:
                return subprocess.CompletedProcess(argv, 1, "", "already exists")
            self.release = {"tag_name": "v0.2.0", "draft": True, "id": 1}
        elif operation == "upload":
            # Mirror gh --clobber: the prior asset is gone if replacement fails.
            name = Path(argv[4]).name
            self.assets.pop(name, None)
            if self.fail_upload == name:
                self.fail_upload = None
                return subprocess.CompletedProcess(argv, 1, "", "upload interrupted")
            self.assets[name] = Path(argv[4]).read_bytes()
        elif operation == "download":
            if self.fail_download:
                return subprocess.CompletedProcess(argv, 1, "", "download interrupted")
            destination = Path(argv[argv.index("--dir") + 1])
            for name, data in self.assets.items():
                if name != self.missing:
                    (destination / name).write_bytes(b"corrupt" if name == self.corrupt else data)
        elif operation == "edit":
            self.release["draft"] = False
        else:
            raise AssertionError(f"Unexpected GitHub command: {operation}")
        return subprocess.CompletedProcess(argv, 0, "", "")

    def mutations(self):
        return [argv[2] for argv in self.history if argv[:2] == ["gh", "release"] and argv[2] != "download"]


class PublisherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source_temp = tempfile.TemporaryDirectory()
        cls.source = Path(cls.source_temp.name)
        for args in (["init", "--quiet"],
                     ["-c", "user.name=CI Test", "-c", "user.email=ci@example.invalid",
                      "-c", "core.hooksPath=", "commit", "--quiet", "--allow-empty", "-m", "fixture"],
                     ["tag", "v0.2.0"]):
            REAL_RUN(["git", *args], cwd=cls.source, check=True, capture_output=True)
        cls.sha = REAL_RUN(["git", "rev-parse", "HEAD"], cwd=cls.source, check=True,
                           capture_output=True, text=True).stdout.strip()

    @classmethod
    def tearDownClass(cls):
        cls.source_temp.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.dist = Path(self.temp.name)
        self.payload = b"synthetic bridge executable\n"
        (self.dist / "rdpilot-bridge.exe").write_bytes(self.payload)
        self.manifest = f"{hashlib.sha256(self.payload).hexdigest()}  rdpilot-bridge.exe\n".encode()
        (self.dist / "SHA256SUMS").write_bytes(self.manifest)

    def run_publisher(self, gh, **overrides):
        args = {"repo": "example/rdpilot", "tag": "v0.2.0", "source_sha": self.sha,
                "dist": self.dist, "source_dir": self.source}
        args.update(overrides)
        with patch.object(publisher.subprocess, "run", side_effect=gh):
            publisher.Publisher(**args).publish()

    def test_new_draft_uploads_verifies_and_publishes(self):
        gh = GitHub()
        self.run_publisher(gh)
        self.assertEqual(gh.mutations(), ["create", "upload", "upload", "edit"])
        self.assertEqual(gh.assets, {"rdpilot-bridge.exe": self.payload, "SHA256SUMS": self.manifest})
        self.assertFalse(gh.release["draft"])

    def test_existing_draft_is_not_recreated(self):
        gh = GitHub(True)
        gh.assets = {"rdpilot-bridge.exe": b"old", "SHA256SUMS": b"old"}
        self.run_publisher(gh)
        self.assertEqual(gh.mutations(), ["upload", "upload", "edit"])

    def test_interrupted_asset_uploads_resume_without_recreation(self):
        for name in ("rdpilot-bridge.exe", "SHA256SUMS"):
            with self.subTest(asset=name):
                gh = GitHub()
                gh.fail_upload = name
                with self.assertRaises(publisher.PublishError):
                    self.run_publisher(gh)
                self.assertTrue(gh.release["draft"])
                self.assertNotIn("edit", gh.mutations())
                self.run_publisher(gh)
                self.assertEqual(gh.mutations().count("create"), 1)
                self.assertEqual(gh.mutations().count("edit"), 1)
                self.assertEqual(gh.assets["rdpilot-bridge.exe"], self.payload)
                self.assertEqual(gh.assets["SHA256SUMS"], self.manifest)

    def test_download_failure_can_resume(self):
        gh = GitHub(True)
        gh.fail_download = True
        with self.assertRaises(publisher.PublishError):
            self.run_publisher(gh)
        self.assertNotIn("edit", gh.mutations())
        gh.fail_download = False
        self.run_publisher(gh)
        self.assertEqual(gh.mutations().count("edit"), 1)

    def test_corrupt_or_missing_downloads_never_publish(self):
        for field in ("corrupt", "missing"):
            for asset in ("rdpilot-bridge.exe", "SHA256SUMS"):
                with self.subTest(field=field, asset=asset):
                    gh = GitHub(True)
                    setattr(gh, field, asset)
                    with self.assertRaises(publisher.PublishError):
                        self.run_publisher(gh)
                    self.assertTrue(gh.release["draft"])
                    self.assertNotIn("edit", gh.mutations())

    def test_published_release_has_no_mutations(self):
        gh = GitHub(False)
        with self.assertRaises(publisher.PublishError):
            self.run_publisher(gh)
        self.assertEqual(gh.mutations(), [])

    def test_state_changes_before_each_mutation_refuse(self):
        for read, allowed in ((2, []), (3, ["upload"]), (4, ["upload", "upload"])):
            for changed in ({"tag_name": "v0.2.0", "draft": False, "id": 1},
                            {"tag_name": "v0.3.0", "draft": True, "id": 1},
                            {"tag_name": "v0.2.0", "draft": True, "id": 2}, None):
                with self.subTest(read=read, changed=changed):
                    gh = GitHub(True)
                    gh.change_at_read[read] = changed
                    with self.assertRaises(publisher.PublishError):
                        self.run_publisher(gh)
                    self.assertEqual(gh.mutations(), allowed)

    def test_lookup_errors_and_invalid_state_fail_closed(self):
        for error in (401, 403, 500, "transport", "malformed"):
            with self.subTest(error=error):
                gh = GitHub(True)
                gh.lookup = error
                with self.assertRaises(publisher.PublishError):
                    self.run_publisher(gh)
                self.assertEqual(gh.mutations(), [])
        for state in ({"tag_name": "v0.2.0", "draft": "true", "id": 1},
                      {"tag_name": "v0.2.0", "draft": True, "id": True},
                      {"tag_name": "v0.3.0", "draft": True, "id": 1}, []):
            with self.subTest(state=state):
                gh = GitHub(True)
                gh.release = state
                with self.assertRaises(publisher.PublishError):
                    self.run_publisher(gh)
                self.assertEqual(gh.mutations(), [])

    def test_source_or_tag_mismatch_prevents_github_calls(self):
        for options in ({"source_sha": "0" * 40}, {"tag": "v0.3.0"}, {"source_sha": "not-a-sha"}):
            with self.subTest(options=options):
                gh = GitHub()
                with self.assertRaises(publisher.PublishError):
                    self.run_publisher(gh, **options)
                self.assertEqual(gh.history, [])

    def test_local_missing_or_wrong_assets_prevent_github_calls(self):
        for name in ("rdpilot-bridge.exe", "SHA256SUMS"):
            path = self.dist / name
            original = path.read_bytes()
            for data in (None, b"wrong"):
                with self.subTest(name=name, data=data):
                    if data is None:
                        path.unlink()
                    else:
                        path.write_bytes(data)
                    gh = GitHub()
                    with self.assertRaises(publisher.PublishError):
                        self.run_publisher(gh)
                    self.assertEqual(gh.history, [])
                    path.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
