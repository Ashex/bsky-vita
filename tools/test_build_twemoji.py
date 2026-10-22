import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from PIL import Image

SPEC = importlib.util.spec_from_file_location("build_twemoji", Path(__file__).with_name("build-twemoji.py"))
BUILDER = importlib.util.module_from_spec(SPEC)
sys.dont_write_bytecode = True
SPEC.loader.exec_module(BUILDER)


class Response:
    status_code = 200
    def __init__(self, content):
        self.content = content
    def raise_for_status(self):
        pass


class Session:
    def __init__(self, failure=False):
        self.failure = failure
    def get(self, url, timeout):
        if self.failure:
            raise BUILDER.requests.ConnectionError("offline")
        image = Image.new("RGBA", (8, 8), (20, 30, 40, 255))
        import io
        stream = io.BytesIO()
        image.save(stream, format="PNG")
        return Response(stream.getvalue())


class GeneratorTests(unittest.TestCase):
    def test_failed_generation_leaves_both_outputs_untouched(self):
        with tempfile.TemporaryDirectory() as temp:
            png, rs = Path(temp) / "atlas.png", Path(temp) / "table.rs"
            png.write_bytes(b"old-png")
            rs.write_bytes(b"old-rs")
            with patch.object(BUILDER, "OUT_PNG", png), patch.object(BUILDER, "OUT_RS", rs), patch.object(BUILDER, "collect_codepoints", return_value=[1]), patch.object(BUILDER, "SEQUENCES", []), patch.object(BUILDER.requests, "Session", return_value=Session(True)):
                with self.assertRaises(BUILDER.requests.ConnectionError):
                    BUILDER.main()
            self.assertEqual(png.read_bytes(), b"old-png")
            self.assertEqual(rs.read_bytes(), b"old-rs")

    def test_generation_is_deterministic_and_emits_matching_outputs(self):
        with tempfile.TemporaryDirectory() as temp:
            png, rs = Path(temp) / "atlas.png", Path(temp) / "table.rs"
            with patch.object(BUILDER, "OUT_PNG", png), patch.object(BUILDER, "OUT_RS", rs), patch.object(BUILDER, "collect_codepoints", return_value=[1]), patch.object(BUILDER, "SEQUENCES", [(2, 3)]), patch.object(BUILDER.requests, "Session", return_value=Session()):
                BUILDER.main()
                first = (png.read_bytes(), rs.read_bytes())
                BUILDER.main()
            self.assertEqual((png.read_bytes(), rs.read_bytes()), first)
            self.assertIn("&[0x000001]", rs.read_text())
            self.assertIn("&[0x000002, 0x000003]", rs.read_text())
            with Image.open(png) as atlas:
                self.assertEqual(atlas.size, (1024, 64))


if __name__ == "__main__":
    unittest.main()
