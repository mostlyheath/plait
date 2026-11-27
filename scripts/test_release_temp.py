from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
import sys
import tempfile
import unittest

from check_test_temp import check


class TempGuardTest(unittest.TestCase):
    def test_detects_replacement_even_when_count_is_unchanged(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            before = directory / "plait-before"
            before.mkdir()
            self.assertEqual(check([sys.executable, "-c", "pass"], [directory]), 0)
            self.assertEqual(check([sys.executable, "-c", "exit(7)"], [directory]), 7)
            replace = """
from pathlib import Path
import sys
original = Path(sys.argv[1])
original.rmdir()
original.with_name("plait-after").mkdir()
"""
            messages = StringIO()
            with redirect_stderr(messages):
                status = check([sys.executable, "-c", replace, str(before)], [directory])
            self.assertEqual(status, 1)
            self.assertIn("plait-after", messages.getvalue())
            self.assertIn("plait-before", messages.getvalue())
