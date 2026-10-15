#!/usr/bin/env python3
"""Exercise the real terminal adapter with a controlling PTY and separate data stdin."""
import fcntl
import os
import pty
import select
import subprocess
import sys
import termios
import time
import unittest

PROBE = os.path.abspath(sys.argv[1])


class Session:
    def __init__(self, confirm=True):
        self.master, self.slave = pty.openpty()
        self.before = termios.tcgetattr(self.slave)
        self.transcript = b""

        def attach():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            [PROBE] + (["confirm"] if confirm else []),
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            pass_fds=(self.slave,), preexec_fn=attach,
        )
        self.process.stdin.write(b"payload")
        self.process.stdin.close()
        self.process.stdin = None

    def __enter__(self):
        self.wait_for("密码: ")
        return self

    def wait_for(self, text):
        marker = text.encode()
        end = time.monotonic() + 10
        while marker not in self.transcript:
            if time.monotonic() > end:
                raise AssertionError("terminal prompt timed out")
            if select.select([self.master], [], [], 0.1)[0]:
                self.transcript += os.read(self.master, 65536)

    def send(self, text):
        os.write(self.master, text.encode())

    def finish(self):
        stdout, stderr = self.process.communicate(timeout=10)
        while select.select([self.master], [], [], 0)[0]:
            self.transcript += os.read(self.master, 65536)
        if termios.tcgetattr(self.slave) != self.before:
            raise AssertionError("terminal attributes were not restored")
        return self.process.returncode, stdout, stderr

    def __exit__(self, *_):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        os.close(self.master)
        os.close(self.slave)


class TerminalTests(unittest.TestCase):
    def confirm(self, first, second):
        with Session() as session:
            session.send(first + "\r")
            session.wait_for("再次输入密码: ")
            session.send(second + "\r")
            code, stdout, stderr = session.finish()
            self.assertEqual(code, 0, stderr)
            self.assertEqual(stderr, b"")
            self.assertIn(b"stdin-bytes=7", stdout)
            self.assertIn(b"*", session.transcript)
            for channel in (session.transcript, stdout, stderr):
                self.assertNotIn("中文".encode(), channel)

    def test_unicode_nfc_backspace_and_separate_stdin(self):
        self.confirm("中文cafe\u0301x\x7f", "中文café")

    def test_backspace_removes_complete_grapheme(self):
        self.confirm("中文e\u0301\x7f密码", "中文密码")

    def test_bracketed_paste(self):
        self.confirm("\x1b[200~中文 安全句子\x1b[201~", "中文 安全句子")

    def test_mismatch_restores_terminal(self):
        with Session() as session:
            session.send("中文甲\r")
            session.wait_for("再次输入密码: ")
            session.send("中文乙\r")
            code, _, stderr = session.finish()
            self.assertEqual(code, 1)
            self.assertIn(b"invalid input", stderr)

    def test_cancel_restores_terminal(self):
        with Session() as session:
            session.send("中文\x03")
            code, _, stderr = session.finish()
            self.assertEqual(code, 1)
            self.assertIn(b"cancelled", stderr)

    def test_oversize_paste_restores_terminal(self):
        with Session() as session:
            session.send("\x1b[200~" + "密" * 1500 + "\x1b[201~")
            code, _, stderr = session.finish()
            self.assertEqual(code, 1)
            self.assertIn(b"invalid input", stderr)

    def test_without_controlling_terminal_fails(self):
        result = subprocess.run([PROBE], input=b"password\n", capture_output=True,
                                start_new_session=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertTrue(result.stderr)
        self.assertEqual(result.stdout, b"")
        self.assertNotIn(b"password", result.stderr)


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0]], verbosity=2)
