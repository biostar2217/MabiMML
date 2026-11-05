"""AI 중계 형식 시험 (네트워크 없이). python -m unittest tests.test_ai_relay"""
import io
import json
import os
import sys
import unittest
import urllib.error

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from mabimml import ai  # noqa: E402

CHOICES = {"rank": ["연습", "1"]}


class Resp(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()


def params():
    return ai.build_request("템포 10% 빠르게", {"speed_percent": 100}, {"열린 곡": "없음"}, None,
                            ai.FIXED_MODEL, ai.FIXED_EFFORT, CHOICES)


class RelayTest(unittest.TestCase):
    def test_body(self):
        body = ai.relay_body(params(), "1.6")
        self.assertEqual(set(body), {"app", "version", "system", "messages", "max_tokens"})
        self.assertEqual(body["app"], "MabiMML")
        self.assertLessEqual(body["max_tokens"], 8192)
        self.assertIn("JSON 스키마", body["system"])
        self.assertIn('"changes"', body["system"])
        self.assertIsInstance(body["messages"][0]["content"], str)
        self.assertIn("명령: 템포 10% 빠르게", body["messages"][0]["content"])
        self.assertNotIn("model", body)          # 모델은 서버가 고른다

    def test_ok(self):
        sent = {}

        def opener(req, timeout=None):
            sent["body"] = json.loads(req.data.decode("utf-8"))
            sent["ua"] = req.get_header("User-agent")
            text = '```json\n{"changes":[{"name":"speed_percent","value":"110"}],"reply":"10% 빠르게 했어요"}\n```'
            return Resp(json.dumps({"ok": True, "text": text, "stop_reason": "end_turn"}).encode())
        data = ai.ask_relay(params(), "1.6", opener=opener)
        self.assertEqual(ai.changes_to_dict(data, CHOICES)["speed_percent"], 110)
        self.assertEqual(sent["ua"], "MabiMML/1.6")

    def test_errors(self):
        def http(code, payload):
            def f(req, timeout=None):
                raise urllib.error.HTTPError(ai.RELAY_URL, code, "x", {}, io.BytesIO(payload))
            return f
        msg = "잠시 뒤 다시 해 주세요"
        with self.assertRaisesRegex(ai.AIError, msg):
            ai.ask_relay(params(), "1.6", opener=http(429, json.dumps(
                {"ok": False, "error": "rate_limited", "message": msg}).encode()))
        with self.assertRaisesRegex(ai.AIError, "502"):
            ai.ask_relay(params(), "1.6", opener=http(502, b"<html>bad gateway</html>"))

        def down(req, timeout=None):
            raise urllib.error.URLError("down")
        with self.assertRaisesRegex(ai.AIError, "연결"):
            ai.ask_relay(params(), "1.6", opener=down)

    def test_extract(self):
        self.assertEqual(ai.extract_json('앞말 {"a": 1} 뒷말'), {"a": 1})
        with self.assertRaises(ai.AIError):
            ai.extract_json("JSON 없음")


if __name__ == "__main__":
    unittest.main()
