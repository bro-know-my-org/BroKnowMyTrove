#!/usr/bin/env python3
"""Production KDF/PTY coverage for backups and credential management; disposable data only."""
import json
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest

helpers = runpy.run_path(str(Path(__file__).with_name("test-cli.py")))
run_cli, CLI, PASSWORD = (helpers[key] for key in ["run_cli", "CLI", "PASSWORD"])


class ManagementTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="bkm-management-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.vault = self.root / "vault"

    def call(self, name, *args, **kwargs):
        return run_cli([name, self.vault, *args], **kwargs)

    def test_backup_deleted_source_recovery_and_existing_target_protection(self):
        terminal = self.call("create", "--recovery", confirm=True)[2]
        recovery = re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal).group()
        source = self.root / "source"
        source.write_bytes(bytes(range(256)) * 4097)
        identity = self.call("import", source, "回收的文件")[0].strip()
        self.call("trash", identity)
        backup, restored = self.root / "backup", self.root / "restored"
        report = json.loads(self.call("backup", backup, "--json")[0])
        self.assertEqual(report["verification"], "full")
        self.assertEqual(report["verified_objects"], 1)
        self.assertIsNone(report["publication_failure"])
        header = (backup / "keys.json").read_bytes()
        self.call("backup", backup, expected=1)
        self.assertEqual((backup / "keys.json").read_bytes(), header)
        shutil.rmtree(self.vault)
        report = json.loads(run_cli(["restore-vault", backup, restored, "--recovery-key", "--json"],
                                    credential=recovery, language="en-US")[0])
        self.assertEqual(report["verification"], "full")
        self.vault = restored
        self.call("restore", identity)
        target = self.root / "export"
        self.call("export", identity, target)
        self.assertEqual(target.read_bytes(), source.read_bytes())
        self.call("mkdir", "恢复后可写")

    def test_copy_is_explicit_and_never_claims_to_authenticate_corrupt_contents(self):
        self.call("create", confirm=True)
        source = self.root / "source"
        source.write_bytes(b"authenticated contents")
        self.call("import", source, "文件")
        obj = next((self.vault / "objects").iterdir())
        damaged = bytearray(obj.read_bytes()); damaged[-1] ^= 1; obj.write_bytes(damaged)
        failed, copied = self.root / "failed", self.root / "copied"
        self.call("backup", failed, expected=1)
        self.assertFalse(failed.exists())
        report = json.loads(self.call("backup", copied, "--verification", "copy", "--json")[0])
        self.assertEqual(report["verification"], "copy")
        self.assertEqual(report["verified_objects"], 0)
        self.assertEqual(json.loads(run_cli(["check", copied, "--json"], expected=1)[0])["failed_objects"], [obj.name])

    def test_password_recovery_rotation_reset_and_removal(self):
        terminal = self.call("create", "--recovery", confirm=True)[2]
        old_key = re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal).group()
        first, second = "新密码 café！", "恢复后密码"
        self.call("change-password", "--confirm", new_password=first)
        self.call("list", credential=PASSWORD, expected=1)
        self.call("list", "--recovery-key", credential=old_key)
        output, error, terminal = self.call("rotate-recovery", "--confirm", credential=first, language="en-US")
        new_key = re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal).group()
        self.assertNotIn(new_key, output + error)
        self.call("list", "--recovery-key", credential=old_key, expected=1)
        self.call("change-password", "--confirm", "--recovery-key", credential=new_key, new_password=second)
        self.call("list", credential=first, expected=1)
        self.call("remove-recovery", "--confirm", credential=second)
        self.call("list", "--recovery-key", credential=new_key, expected=1)
        self.call("mkdir", "凭据更换后可写", credential=second)

    def test_credential_commands_require_confirmation_before_terminal_or_writes(self):
        for name in ["change-password", "rotate-recovery", "remove-recovery"]:
            result = subprocess.run([CLI, name, str(self.vault)], capture_output=True,
                                    start_new_session=True, timeout=10)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stdout, b"")
        self.assertFalse(self.vault.exists())
        for name in ["rotate-recovery", "remove-recovery"]:
            for extra in [["--recovery-key"], ["--confirm", "--recovery-key"]]:
                result = subprocess.run([CLI, name, str(self.vault), *extra],
                                        capture_output=True, start_new_session=True, timeout=10)
                self.assertEqual(result.returncode, 2)

    def test_recovery_display_failure_reports_update_and_password_still_works(self):
        self.call("create", confirm=True)
        output, error, _ = self.call("rotate-recovery", "--confirm", expected=1,
                                     disconnect_terminal=True)
        self.assertEqual(output, "")
        self.assertIn("恢复入口已更换，但新密钥显示失败", error)
        self.call("list")
        terminal = self.call("rotate-recovery", "--confirm")[2]
        key = re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal).group()
        self.call("list", "--recovery-key", credential=key)


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0], *sys.argv[2:]], verbosity=2)
