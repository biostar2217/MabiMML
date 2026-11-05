"""자동 업데이트 모듈 시험 (네트워크 없이). python -m unittest tests.test_updater"""
import io
import json
import os
import sys
import tempfile
import unittest
import urllib.error

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from mabimml import updater  # noqa: E402


class FakeResp(io.BytesIO):
    def __init__(self, data, length=None):
        super().__init__(data)
        self.headers = {"Content-Length": str(len(data) if length is None else length)}

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()


def opener_json(obj):
    return lambda req, timeout=None: FakeResp(json.dumps(obj).encode("utf-8"))


def opener_raise(exc):
    def f(req, timeout=None):
        raise exc
    return f


RELEASE = {
    "tag_name": "v1.5", "html_url": "https://github.com/biostar2217/MabiMML/releases/tag/v1.5",
    "body": "고친 점", "assets": [
        {"name": "소스.zip", "browser_download_url": "https://x/src.zip", "size": 10},
        {"name": "MabiMML_v1.5_PDF.exe", "browser_download_url": "https://x/a.exe", "size": 123},
    ]}


class VersionTest(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(updater.parse_version("v1.4"), (1, 4, 0))
        self.assertEqual(updater.parse_version("1.4.2"), (1, 4, 2))
        self.assertIsNone(updater.parse_version("latest"))

    def test_newer(self):
        self.assertTrue(updater.is_newer("v1.5", "1.4"))
        self.assertTrue(updater.is_newer("v1.10", "1.9"))
        self.assertTrue(updater.is_newer("v2.0", "1.99"))
        self.assertTrue(updater.is_newer("v1.4.1", "1.4"))
        self.assertFalse(updater.is_newer("v1.4", "1.4"))
        self.assertFalse(updater.is_newer("v1.4.0", "1.4"))
        self.assertFalse(updater.is_newer("v1.3", "1.4"))
        self.assertFalse(updater.is_newer("nightly", "1.4"))


class AssetTest(unittest.TestCase):
    def test_pick(self):
        a = updater.pick_asset(RELEASE["assets"])
        self.assertEqual(a["name"], "MabiMML_v1.5_PDF.exe")
        self.assertEqual(a["size"], 123)

    def test_none(self):
        self.assertIsNone(updater.pick_asset([{"name": "MabiMML_v1.5.exe", "browser_download_url": "u"}]))
        self.assertIsNone(updater.pick_asset([{"name": "MabiMML_v1.5_PDF.exe.zip", "browser_download_url": "u"}]))
        self.assertIsNone(updater.pick_asset(None))


class CheckTest(unittest.TestCase):
    def test_new(self):
        r = updater.check("1.4", opener=opener_json(RELEASE))
        self.assertEqual(r.status, "new")
        self.assertEqual(r.tag, "v1.5")
        self.assertEqual(r.asset["url"], "https://x/a.exe")

    def test_latest(self):
        self.assertEqual(updater.check("1.5", opener=opener_json(RELEASE)).status, "latest")

    def test_404(self):
        err = urllib.error.HTTPError(updater.API_LATEST, 404, "Not Found", {}, None)
        r = updater.check("1.4", opener=opener_raise(err))
        self.assertEqual(r.status, "unknown")
        self.assertIn("릴리스", r.reason)

    def test_network_fail(self):
        for exc in (urllib.error.URLError("down"), TimeoutError(), OSError("x")):
            self.assertEqual(updater.check("1.4", opener=opener_raise(exc)).status, "unknown")

    def test_bad_json(self):
        bad = lambda req, timeout=None: FakeResp(b"<html>")
        self.assertEqual(updater.check("1.4", opener=bad).status, "unknown")
        self.assertEqual(updater.check("1.4", opener=opener_json(["x"])).status, "unknown")


class DueTest(unittest.TestCase):
    def test_due(self):
        now = 1_000_000.0
        self.assertTrue(updater.due({}, now))
        self.assertFalse(updater.due({updater.CFG_LAST: now - 3600}, now))
        self.assertTrue(updater.due({updater.CFG_LAST: now - 90000}, now))
        self.assertFalse(updater.due({updater.CFG_ENABLED: False}, now))
        self.assertTrue(updater.due({updater.CFG_LAST: "garbage"}, now))


class DownloadTest(unittest.TestCase):
    def test_size_check(self):
        data = b"x" * (2 * 1024 * 1024)
        op = lambda req, timeout=None: FakeResp(data)
        with tempfile.TemporaryDirectory() as d:
            dest = os.path.join(d, "a.exe")
            seen = []
            updater.download("https://x/a.exe", dest, len(data), progress=lambda g, t: seen.append((g, t)), opener=op)
            self.assertEqual(os.path.getsize(dest), len(data))
            self.assertEqual(seen[-1], (len(data), len(data)))
            with self.assertRaises(OSError):
                updater.download("https://x/a.exe", dest, len(data) + 1, opener=op)

    def test_script(self):
        s = updater.swap_script(r"C:\t\new.exe", r"D:\앱\MabiMML.exe", r"D:\앱\MabiMML_이전.exe")
        self.assertIn('move /y "%TARGET%" "%BACKUP%"', s)
        self.assertIn(":restore", s)
        self.assertIn('start "" "%TARGET%"', s)


if __name__ == "__main__":
    unittest.main()
