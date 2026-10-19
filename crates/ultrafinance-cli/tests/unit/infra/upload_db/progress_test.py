"""Verify the remote reporter independently of Docker or AWS."""
import pathlib
import runpy
import tempfile
import unittest

MODULE = pathlib.Path(__file__).resolve().parents[4] / "src/infra/upload-progress.py"
REPORTER = runpy.run_path(str(MODULE))


class ProgressTests(unittest.TestCase):
    def test_download_counts_partial_bytes_but_requires_checksum_for_completion(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            dump = root / "dump"
            verified = root / "verified"
            dump.mkdir()
            verified.mkdir()
            manifest = root / "downloads"
            manifest.write_text("123.dat\thash\t100\tSECRET_URL\n")
            (dump / "123.dat").write_bytes(b"x" * 30)
            progress = REPORTER["download_position"]
            self.assertEqual(progress(manifest, dump, verified), (30, 100))
            (dump / "123.dat").write_bytes(b"x" * 100)
            self.assertEqual(progress(manifest, dump, verified), (99, 100))
            (verified / "123.dat").touch()
            self.assertEqual(progress(manifest, dump, verified), (100, 100))

    def test_restore_deduplicates_completed_objects_and_never_claims_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            toc = root / "toc.list"
            log = root / "restore.log"
            toc.write_text("; header\n\n1; TABLE secret\n2; TABLE DATA secret\n3; INDEX secret\n")
            log.write_text("pg_restore: finished item 2 TABLE DATA secret\n" * 2)
            progress = REPORTER["restore_position"]
            self.assertEqual(progress(log, toc), (1, 3))
            log.write_text("".join(f"pg_restore: finished item {i} SECRET\n" for i in range(1, 4)))
            self.assertEqual(progress(log, toc), (2, 3))


if __name__ == "__main__":
    unittest.main()
