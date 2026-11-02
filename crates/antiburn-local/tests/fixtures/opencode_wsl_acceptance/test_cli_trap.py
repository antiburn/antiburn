"""Check the WSL acceptance command allowlist without WSL or OpenCode."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from server import install_cli_trap

SAFETY_FLAGS = (
    "OPENCODE_DISABLE_AUTOUPDATE",
    "OPENCODE_DISABLE_MODELS_FETCH",
    "OPENCODE_DISABLE_DEFAULT_PLUGINS",
    "OPENCODE_SKIP_MIGRATIONS",
)


class CliTrapTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.home = Path(directory.name)
        self.args = install_cli_trap(self.home, 1_800_000_000)
        self.trap = self.home / ".opencode/bin/opencode"
        self.trap_before = self.trap.read_bytes()
        self.environment = {
            **os.environ,
            "HOME": str(self.home),
            **{name: "true" for name in SAFETY_FLAGS},
        }

    def run_trap(self, args, environment=None):
        return subprocess.run(
            [str(self.trap), *args],
            env=self.environment if environment is None else environment,
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )

    def assert_denied(self, args, environment=None):
        (self.home / "unexpected-opencode-command").unlink(missing_ok=True)
        result = self.run_trap(args, environment)
        self.assertEqual(result.returncode, 97)
        self.assertEqual(result.stdout, "")
        self.assertTrue((self.home / "unexpected-opencode-command").exists())
        self.assertFalse((self.home / "opencode-commands.jsonl").exists())
        self.assertEqual(self.trap.read_bytes(), self.trap_before)

    def test_exact_metadata_probe_returns_empty_history_and_records_arguments(self):
        result = self.run_trap(self.args)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), [])
        self.assertEqual(
            (self.home / "opencode-commands.jsonl").read_text().splitlines(),
            [json.dumps(self.args)],
        )
        self.assertFalse((self.home / "unexpected-opencode-command").exists())
        self.assertEqual(self.trap.read_bytes(), self.trap_before)

    def test_commands_outside_the_empty_history_allowlist_fail(self):
        for args in (
            [],
            ["--pure", "service", "start"],
            ["--pure", "api", "get", "/api/info"],
            ["--pure", "run", "synthetic request"],
            ["--pure", "upgrade"],
            ["--pure", "install"],
            ["--pure", "export", "ses_unexpected"],
            ["--pure", "db", "DELETE FROM session", "--format", "json"],
            [
                "--pure",
                "db",
                self.args[2] + "; DELETE FROM session",
                "--format",
                "json",
            ],
            self.args[1:],
            [*self.args, "--unexpected"],
        ):
            with self.subTest(args=args):
                self.assert_denied(args)

    def test_missing_or_disabled_safety_flags_fail(self):
        for name in SAFETY_FLAGS:
            for value in (None, "false"):
                environment = self.environment.copy()
                environment.pop(name)
                if value is not None:
                    environment[name] = value
                with self.subTest(flag=name, value=value):
                    self.assert_denied(self.args, environment)


if __name__ == "__main__":
    unittest.main()
