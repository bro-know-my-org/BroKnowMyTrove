#!/usr/bin/env python3
"""Real CLI processes, isolated controlling terminals, real production KDF, disposable data."""
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import subprocess
import sys
import tempfile
import termios
import time
import unittest

CLI = os.path.abspath(sys.argv[1])
PASSWORD = "中文端到端café"


def run_cli(args, *, credential=PASSWORD, confirm=False, locale="C.UTF-8", expected=0, during=None,
            stdout_sink=None, disconnect_terminal=False, stdin_source=None, stderr_sink=None):
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    transcript = b""

    def attach():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        if disconnect_terminal:
            signal.signal(signal.SIGHUP, signal.SIG_IGN)

    env = dict(os.environ, LC_ALL=locale)
    env.pop("DISPLAY", None)
    env.pop("WAYLAND_DISPLAY", None)
    process = subprocess.Popen(
        [CLI, *map(str, args)], stdin=stdin_source if stdin_source is not None else subprocess.DEVNULL,
        stdout=stdout_sink if stdout_sink is not None else subprocess.PIPE,
        stderr=stderr_sink if stderr_sink is not None else subprocess.PIPE, env=env,
        pass_fds=(slave,), preexec_fn=attach,
    )

    def wait(marker):
        nonlocal transcript
        end = time.monotonic() + 15
        while marker.encode() not in transcript:
            if time.monotonic() > end or process.poll() is not None:
                raise AssertionError("CLI terminal prompt unavailable")
            if select.select([master], [], [], 0.1)[0]:
                transcript += os.read(master, 65536)

    try:
        wait("恢复密钥: " if "--recovery-key" in args else "密码: ")
        os.write(master, (credential + "\r").encode())
        if confirm:
            wait("再次输入密码: ")
            os.write(master, (PASSWORD + "\r").encode())
        if disconnect_terminal:
            wait("\x1b[?2004l")
            end = time.monotonic() + 2
            while termios.tcgetattr(slave) != before:
                if time.monotonic() > end:
                    raise AssertionError("terminal not restored before disconnect")
                time.sleep(0.001)
            os.close(master)
            master = None
        if during:
            during(process)
        stdout, stderr = process.communicate(timeout=20)
        stdout = stdout or b""
        stderr = stderr or b""
        while master is not None and select.select([master], [], [], 0)[0]:
            transcript += os.read(master, 65536)
        if process.returncode != expected:
            raise AssertionError(f"CLI {args[0]} exit={process.returncode}, expected={expected}; {stderr!r}")
        if master is not None and termios.tcgetattr(slave) != before:
            raise AssertionError("CLI did not restore terminal attributes")
        secrets = [credential, PASSWORD] if confirm else [credential]
        if any(secret.encode() in transcript + stdout + stderr for secret in secrets):
            raise AssertionError("CLI exposed its credential")
        return stdout.decode(), stderr.decode(), transcript.decode()
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        for pipe in [process.stdout, process.stderr]:
            if pipe is not None:
                pipe.close()
        if master is not None:
            os.close(master)
        os.close(slave)


class CliTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="bkm-cli-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.vault = self.root / "vault"

    def call(self, command, *args, **kwargs):
        return run_cli([command, self.vault, *args], **kwargs)

    def test_cross_process_unicode_recovery_and_file_lifecycle(self):
        stdout, stderr, terminal = self.call(
            "create", "--recovery", confirm=True, locale="C",
            credential="中文端到端cafe\u0301",
        )
        recovery = re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal).group()
        self.assertNotIn(recovery, stdout + stderr)
        directory = self.call("mkdir", "资料目录")[0].strip()
        source = self.root / "source"
        source.write_bytes(bytes(range(256)) * 8193)
        file_id = self.call("import", source, "中文资料", "--parent", directory)[0].strip()
        listing = [json.loads(line) for line in self.call("list", locale="C")[0].splitlines()]
        self.assertEqual({e["id"] for e in listing}, {directory, file_id})
        self.assertEqual(next(e for e in listing if e["id"] == file_id)["parent"], directory)
        self.assertEqual(json.loads(self.call("search", "中文")[0])["id"], file_id)
        target = self.root / "导出"
        self.call("export", file_id, target, "--recovery-key", credential=recovery)
        self.assertEqual(target.read_bytes(), source.read_bytes())
        original = target.read_bytes()
        source.write_bytes(b"replacement")
        self.call("replace", file_id, source)
        self.call("export", file_id, target, expected=1)
        self.assertEqual(target.read_bytes(), original)
        replacement = self.root / "replacement-export"
        self.call("export", file_id, replacement)
        self.assertEqual(replacement.read_bytes(), source.read_bytes())
        self.call("rename", file_id, "更名资料")
        self.call("move", file_id)
        self.call("trash", file_id)
        self.assertEqual(json.loads(self.call("list", "--trash")[0])["id"], file_id)
        self.call("restore", file_id)
        self.call("check")
        self.call("trash", file_id)
        self.assertIn("trash_object_bytes", self.call("space")[0])
        self.call("empty-trash", "--confirm")
        self.call("gc", "--confirm")
        self.assertEqual(len(list((self.vault / "objects").iterdir())), 0)
        self.assertEqual(source.read_bytes(), b"replacement")

    def test_corruption_inspection_explicit_recovery_and_wrong_password(self):
        self.call("create", confirm=True)
        source = self.root / "source"
        source.write_bytes(b"intact historic version")
        file_id = self.call("import", source, "保留资料")[0].strip()
        before = set((self.vault / "commits").iterdir())
        report = self.call("inspect")[0]
        head = re.search(r"head: Some\(\s*([0-9a-f]{32})", report).group(1)
        self.call("rename", file_id, "新名称")
        newest = (set((self.vault / "commits").iterdir()) - before).pop()
        newest.write_bytes(b"damaged new commit")
        self.call("list", expected=1)
        self.assertIn("DamagedCommit", self.call("inspect", expected=1)[0])
        target = self.root / "recovered"
        self.call("export", file_id, target, "--commit", head)
        self.assertEqual(target.read_bytes(), source.read_bytes())
        stdout, stderr, _ = self.call("list", credential="错误密码", expected=1)
        self.assertEqual(stdout, "")
        self.assertIn("authentication failed", stderr)
        self.assertNotIn("保留资料", stderr)

    def test_json_results_and_failed_content_report(self):
        self.assertEqual(json.loads(self.call("create", "--json", confirm=True)[0]), {"ok": True})
        directory = json.loads(self.call("mkdir", "目录", "--json")[0])["id"]
        source = self.root / "source"
        source.write_bytes(b"authenticated content")
        file_id = json.loads(self.call("import", source, "资料", "--parent", directory,
                                       "--json")[0])["id"]
        entries = json.loads(self.call("list", "--json")[0])["entries"]
        self.assertEqual({e["id"] for e in entries}, {directory, file_id})
        self.assertGreater(json.loads(self.call("space", "--json")[0])["active_object_bytes"], 0)
        inspection = json.loads(self.call("inspect", "--json")[0])
        self.assertRegex(inspection["head"], r"^[0-9a-f]{32}$")
        self.assertEqual(inspection["issues"], [])
        report = json.loads(self.call("check", "--json")[0])
        self.assertEqual(report, {"verified_objects": 1, "plaintext_bytes": len(source.read_bytes()),
                                  "failed_objects": []})
        cleanup = json.loads(self.call("gc", "--confirm", "--json")[0])
        self.assertIsNone(cleanup["failure"])
        self.assertFalse(cleanup["trash_committed"])
        obj = next((self.vault / "objects").iterdir())
        damaged = bytearray(obj.read_bytes())
        damaged[-1] ^= 1
        obj.write_bytes(damaged)
        report = json.loads(self.call("check", "--json", expected=1)[0])
        self.assertEqual(report["failed_objects"], [obj.name])

    def test_required_confirmation_and_no_controlling_terminal(self):
        for command in ["empty-trash", "gc"]:
            result = subprocess.run([CLI, command, str(self.vault)], capture_output=True,
                                    start_new_session=True, timeout=10)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stdout, b"")
        result = subprocess.run([CLI, "create", str(self.vault)], input=b"secret\n",
                                capture_output=True, start_new_session=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(self.vault.exists())
        self.assertNotIn(b"secret", result.stderr)

    def test_batch_single_unlock_stdin_and_partial_failure(self):
        terminal = self.call("create", "--recovery", confirm=True)[2]
        recovery = re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal).group()
        source = self.root / "source"
        source.write_bytes(b"batch content")
        plan = self.root / "commands.jsonl"
        plan.write_text("\n".join(json.dumps(c) for c in [
            ["import", str(source), "批量资料"], ["list"], ["check"], ["space"],
        ]), encoding="utf-8")
        with plan.open("rb") as data:
            stdout, stderr, terminal = self.call("batch", "-", "--recovery-key",
                                                 credential=recovery, stdin_source=data)
        results = [json.loads(line) for line in stdout.splitlines()]
        self.assertEqual([r["index"] for r in results], [1, 2, 3, 4])
        self.assertTrue(all(r["status"] == "ok" and r["error"] is None for r in results))
        file_id = results[0]["result"]["id"]
        self.assertEqual(results[1]["result"]["entries"][0]["id"], file_id)
        self.assertEqual(terminal.count("\x1b[?2004h"), 1)
        self.assertEqual(stderr, "")
        plan.write_text("\n".join(json.dumps(c) for c in [
            ["rename", file_id, "已完成"], ["rename", "0" * 32, "失败名称"], ["mkdir", "不应执行"],
        ]), encoding="utf-8")
        stdout, stderr, terminal = self.call("batch", plan, expected=1)
        results = [json.loads(line) for line in stdout.splitlines()]
        self.assertEqual([r["status"] for r in results], ["ok", "error"])
        self.assertIsNone(results[1]["result"])
        self.assertEqual(terminal.count("\x1b[?2004h"), 1)
        self.assertNotIn("失败名称", stderr)
        self.assertEqual(json.loads(self.call("list")[0])["name"], "已完成")

    def test_batch_preflight_and_historic_readonly_session(self):
        self.call("create", confirm=True)
        self.call("mkdir", "历史目录")
        head = json.loads(self.call("inspect", "--json")[0])["head"]
        self.call("mkdir", "最新目录")
        plan = self.root / "commands.jsonl"
        plan.write_text('["list"]\n["check"]\n', encoding="utf-8")
        stdout = self.call("batch", plan, "--commit", head)[0]
        results = [json.loads(line) for line in stdout.splitlines()]
        self.assertEqual([e["name"] for e in results[0]["result"]["entries"]], ["历史目录"])
        for data, extra in [
            ('["mkdir","不得执行"]\n["gc"]\n', []),
            ('["mkdir","不得执行"]\n', ["--commit", head]),
        ]:
            result = subprocess.run([CLI, "batch", str(self.vault), "-", *extra],
                                    input=data.encode(), capture_output=True,
                                    start_new_session=True, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stdout, b"")
            self.assertNotIn("不得执行".encode(), result.stderr)
        self.assertEqual(len(self.call("list")[0].splitlines()), 2)

    @unittest.skipUnless(sys.platform == "linux", "requires /dev/full")
    def test_batch_output_failure_stops_after_committed_step(self):
        self.call("create", confirm=True)
        plan = self.root / "commands.jsonl"
        plan.write_text('["mkdir","已提交"]\n["mkdir","不应执行"]\n', encoding="utf-8")
        with open("/dev/full", "wb", buffering=0) as sink:
            _, stderr, _ = self.call("batch", plan, expected=1, stdout_sink=sink)
        self.assertIn("此前完成的操作不会回滚", stderr)
        self.assertEqual(json.loads(self.call("list")[0])["name"], "已提交")

    @unittest.skipUnless(sys.platform == "linux", "requires Linux pipe capacity queries")
    def test_batch_cancel_with_unread_stdout_releases_vault_lock(self):
        self.call("create", confirm=True)
        plan = self.root / "commands.jsonl"
        commands = [["mkdir", f"{i:02d}" + "x" * 220] for i in range(32)]
        plan.write_text("\n".join(json.dumps(c) for c in commands), encoding="utf-8")
        self.call("batch", plan)
        plan.write_text('["list"]\n', encoding="utf-8")

        def cancel_blocked_writer(process):
            capacity = fcntl.fcntl(process.stdout, fcntl.F_SETPIPE_SZ, 4096)
            end = time.monotonic() + 20
            while True:
                unread = int.from_bytes(fcntl.ioctl(process.stdout, termios.FIONREAD, b"\0" * 4),
                                        sys.byteorder)
                # The sole list record exceeds this capacity and cannot complete
                # without a reader, even if an atomic write stalls before 100% full.
                if unread >= capacity // 2:
                    break
                if process.poll() is not None:
                    error = process.stderr.read() if process.stderr is not None else b"merged output"
                    raise AssertionError(f"batch exited before output blocked: {process.returncode}, "
                                         f"unread={unread}, capacity={capacity}, {error!r}")
                if time.monotonic() > end:
                    raise AssertionError(f"batch did not fill its unread output pipe: {unread}/{capacity}")
                time.sleep(0.002)
            process.send_signal(signal.SIGINT)
            end = time.monotonic() + 5
            while process.poll() is None and time.monotonic() < end:
                time.sleep(0.002)
            self.assertEqual(process.poll(), 130, "batch must exit before stdout is drained")
            self.call("mkdir", f"取消后锁已释放{merged}")

        for merged in [False, True]:
            with self.subTest(merged_stderr=merged):
                _, stderr, _ = self.call("batch", plan, expected=130, during=cancel_blocked_writer,
                                          stderr_sink=subprocess.STDOUT if merged else None)
                if not merged:
                    self.assertIn("此前完成的操作不会回滚", stderr)

    def test_real_signals_process_lock_and_restart_after_abrupt_exit(self):
        self.call("create", confirm=True)
        source = self.root / "large-source"
        with source.open("wb") as file:
            file.truncate(256 * 1024 * 1024)

        def interrupt(process, action, existing_temporary):
            end = time.monotonic() + 20
            while True:
                if process.poll() is not None or time.monotonic() > end:
                    raise AssertionError("did not observe active stream before interruption")
                os.kill(process.pid, signal.SIGSTOP)
                try:
                    # A signal being sent is not acknowledgement that the process stopped.
                    while True:
                        pid, status = os.waitpid(process.pid, os.WUNTRACED | os.WNOHANG)
                        if pid:
                            if not os.WIFSTOPPED(status):
                                process.returncode = os.waitstatus_to_exitcode(status)
                                raise AssertionError("import exited before interruption")
                            break
                        if time.monotonic() > end:
                            raise AssertionError("import did not acknowledge SIGSTOP")
                        time.sleep(0.001)
                    # All namespace/size observations are now made while the importer is frozen.
                    growing = any(path not in existing_temporary and path.stat().st_size > 1048576
                                  for path in (self.vault / "objects").glob(".tmp-*"))
                    if growing:
                        self.assertFalse(any(not path.name.startswith(".tmp-")
                                             for path in (self.vault / "objects").iterdir()))
                        if action == signal.SIGINT:
                            _, stderr, _ = self.call("mkdir", "并发拒绝", expected=1)
                            self.assertIn("another local process", stderr)
                        os.kill(process.pid, action)
                        return
                finally:
                    if process.poll() is None:
                        os.kill(process.pid, signal.SIGCONT)
                time.sleep(0.002)

        plan = self.root / "commands.jsonl"
        plan.write_text(json.dumps(["import", str(source), "未提交文件"]) + '\n["mkdir","不应执行"]\n',
                        encoding="utf-8")
        for action, code, args in [(signal.SIGINT, 130, ("import", source, "未提交文件")),
                                   (signal.SIGKILL, -signal.SIGKILL, ("import", source, "未提交文件")),
                                   (signal.SIGINT, 130, ("batch", plan))]:
            existing_temporary = set((self.vault / "objects").glob(".tmp-*"))
            self.call(*args, expected=code,
                      during=lambda process: interrupt(process, action, existing_temporary))
            self.assertEqual(self.call("list")[0], "")
        self.assertTrue(any((self.vault / "objects").glob(".tmp-*")))
        source.write_bytes(b"restart from beginning")
        file_id = self.call("import", source, "重新导入")[0].strip()
        self.call("export", file_id, self.root / "result")
        self.assertEqual((self.root / "result").read_bytes(), source.read_bytes())
        self.call("gc", "--confirm")
        self.assertFalse(any((self.vault / "objects").glob(".tmp-*")))

    @unittest.skipUnless(sys.platform == "linux", "requires /dev/full")
    def test_recovery_key_delivery_survives_status_output_failure(self):
        with open("/dev/full", "wb", buffering=0) as sink:
            _, stderr, terminal = self.call("create", "--recovery", confirm=True,
                                            expected=1, stdout_sink=sink)
        self.assertIn("结果输出失败", stderr)
        recovery = re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal).group()
        self.assertEqual(self.call("list", "--recovery-key", credential=recovery)[0], "")

    def test_recovery_terminal_failure_reports_the_committed_vault(self):
        stdout, stderr, terminal = self.call("create", "--recovery", confirm=True,
                                             expected=1, disconnect_terminal=True)
        self.assertEqual(stdout, "")
        self.assertIn("保险库已创建，但恢复密钥显示失败", stderr)
        self.assertIn("仍可使用密码打开", stderr)
        self.assertIsNone(re.search(r"[0-9a-f]{64}-[0-9a-f]{8}", terminal + stdout + stderr))
        self.assertEqual(self.call("list")[0], "")


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0], *sys.argv[2:]], verbosity=2)
