"""Checksum and atomic-publication checks without a network connection."""
import hashlib
import io
import json
import pathlib
import tempfile
import unittest
from unittest.mock import patch

import download_embedding_model


class DownloadModelTests(unittest.TestCase):
    def test_verified_artifact_is_published_and_reused(self):
        content = b"verified artifact"
        manifest = json.dumps({
            "repository": "test/model", "revision": "pinned",
            "files": {"model.onnx": hashlib.sha256(content).hexdigest()},
        })
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            with patch.object(pathlib.Path, "read_text", return_value=manifest), patch(
                "urllib.request.urlopen", return_value=io.BytesIO(content)
            ) as request:
                download_embedding_model.download(directory)
                download_embedding_model.download(directory)
            self.assertEqual(request.call_count, 1)
            self.assertEqual((directory / "model.onnx").read_bytes(), content)
            self.assertFalse((directory / "model.onnx.download").exists())

    def test_untrusted_download_preserves_existing_file_and_removes_temporary(self):
        manifest = json.dumps({
            "repository": "test/model", "revision": "pinned",
            "files": {"model.onnx": hashlib.sha256(b"trusted").hexdigest()},
        })
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            (directory / "model.onnx").write_bytes(b"existing")
            with patch.object(pathlib.Path, "read_text", return_value=manifest), patch(
                "urllib.request.urlopen", return_value=io.BytesIO(b"untrusted")
            ):
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    download_embedding_model.download(directory)
            self.assertEqual((directory / "model.onnx").read_bytes(), b"existing")
            self.assertFalse((directory / "model.onnx.download").exists())


if __name__ == "__main__":
    unittest.main()
