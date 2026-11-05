"""'불러오는 중' 진행 계산 시험. python -m unittest tests.test_progress"""
import os
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
from mabimml import progress  # noqa: E402

LOG = os.path.join(ROOT, "tests", "data", "audiveris_log_sample.txt")


class AudiverisLogTest(unittest.TestCase):
    def test_real_log(self):
        """실제 Audiveris 로그(5장)를 넣으면 진행률이 줄지 않고 올라가며 끝에서 0.98."""
        meter = progress.AudiverisProgress()
        seen = []
        with open(LOG, encoding="utf-8") as f:
            for line in f:
                if meter.feed(line):
                    seen.append(meter.fraction())
        self.assertEqual(meter.sheets, 5)
        values = [v for v in seen if v is not None]
        self.assertGreater(len(values), 50)
        self.assertEqual(values, sorted(values))          # 뒤로 가지 않는다
        self.assertEqual(values[-1], 0.98)
        self.assertLess(max(v for v in values if v != 0.98), 0.98)

    def test_middle(self):
        meter = progress.AudiverisProgress()
        meter.feed("INFO  [x]    Book 560  | 4 sheets in C:/a.pdf")
        self.assertIsNone(meter.fraction())
        meter.feed("INFO  [x#1]  StepMonitoring 98   | LOAD")
        meter.feed("INFO  [x#2]  StepMonitoring 98   | LOAD")     # 2장 시작 = 1장 끝
        self.assertAlmostEqual(meter.fraction(), 0.25)
        self.assertEqual(meter.current_sheet, 2)

    def test_hint_and_noise(self):
        meter = progress.AudiverisProgress(pages_hint=2)
        self.assertFalse(meter.feed("WARN  something else"))
        meter.feed("INFO  [x#1]  StepMonitoring 98   | PAGE")
        self.assertAlmostEqual(meter.fraction(), 19 / 40)


class EstimateTest(unittest.TestCase):
    def test_time_fraction_caps(self):
        self.assertAlmostEqual(progress.time_fraction(5, 10), 0.5)
        self.assertEqual(progress.time_fraction(50, 10), 0.95)
        self.assertIsNone(progress.time_fraction(5, 0))

    def test_remaining(self):
        self.assertIsNone(progress.remaining(0.5, None))             # 처음엔 '예상 계산 중'
        self.assertAlmostEqual(progress.remaining(10, 0.5), 10)
        self.assertAlmostEqual(progress.remaining(2, None, expected=10), 8)
        self.assertIsNone(progress.remaining(12, None, expected=10))  # 예상을 넘기면 모름
        self.assertEqual(progress.fmt_time(75), "1:15")

    def test_average(self):
        self.assertEqual(progress.update_average(None, 6), 6)
        self.assertAlmostEqual(progress.update_average(10, 5, 0.4), 8)

    def test_pdf_pages(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "a.pdf")
            with open(p, "wb") as f:
                f.write(b"%PDF-1.4 << /Type /Pages /Kids [1 0 R 2 0 R 3 0 R] /Count 3 >> "
                        b"<< /Type /Page >> << /Type /Page >> << /Type /Page >>")
            self.assertEqual(progress.count_pdf_pages(p), 3)
            self.assertEqual(progress.count_pdf_pages(os.path.join(d, "x.png")), 1)
            self.assertEqual(progress.count_pdf_pages(os.path.join(d, "none.pdf")), 0)


class ProgressStateTest(unittest.TestCase):
    def test_overall(self):
        p = progress.Progress(["PDF 인식 중", "악보 읽는 중", "MML 만드는 중"])
        p.stage(0)
        self.assertIsNone(p.snapshot()[3])
        p.set(0.5, "2/4장 인식 중")
        stage, n, name, overall, _, _, detail = p.snapshot()
        self.assertEqual((stage, n, name, detail), (0, 3, "PDF 인식 중", "2/4장 인식 중"))
        self.assertAlmostEqual(overall, 0.5 / 3)
        p.stage(2)
        self.assertEqual(p.snapshot()[2], "MML 만드는 중")

    def test_weights(self):
        p = progress.Progress(["PDF 인식 중", "악보 읽는 중", "MML 만드는 중"], [90, 4, 6])
        p.stage(0)
        p.set(0.5)
        self.assertAlmostEqual(p.snapshot()[3], 0.45)
        p.stage(2)
        p.set(0.5)
        self.assertAlmostEqual(p.snapshot()[3], 0.97)

    def test_cancel_kills_own_process(self):
        p = progress.Progress(["x"])
        proc = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        p.add_process(proc)
        t = time.monotonic()
        p.cancel()
        proc.wait(timeout=15)
        self.assertLess(time.monotonic() - t, 15)
        with self.assertRaises(progress.Cancelled):
            p.check()


if __name__ == "__main__":
    unittest.main()
