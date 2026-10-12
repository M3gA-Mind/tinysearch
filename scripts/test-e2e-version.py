"""Pin module version selection with reordered workspace metadata."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

class VersionTests(unittest.TestCase):
    def test_module_version_is_selected_by_package_name(self):
        """A contract or helper package never supplies the module archive version."""
        script = Path(__file__).with_name("test-e2e.sh").read_text()
        query = re.search(r"jq -er '([^']+)'", script)[1]
        result = subprocess.run(["jq", "-er", query], input=json.dumps({"packages": [
            {"name": "unrelated-contract", "version": "9.9.9"},
            {"name": "tinysearch", "version": "0.1.2-beta.1"}]}),
            text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "0.1.2-beta.1")

    def test_distro_powershell_forwards_arguments_and_exit_status(self):
        """The Windows entrypoint delegates offline to the Bash distro wrapper."""
        script = Path(__file__).with_name("test-distro.ps1").resolve()
        self.assertTrue(script.is_file())
        text = script.read_text()
        self.assertIn('& bash $script @args', text)
        self.assertIn('$exitCode = $LASTEXITCODE', text)
        self.assertIn('exit $exitCode', text)

        pwsh = shutil.which("pwsh")
        if pwsh is None:
            return

        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            bindir = fixture / "bin"
            bindir.mkdir()
            captured = fixture / "arguments.txt"
            fake_bash = bindir / ("bash.cmd" if os.name == "nt" else "bash")
            if os.name == "nt":
                fake_bash.write_text(
                    '@echo off\necho %* > "%CAPTURED_ARGS%"\nexit /b %FAKE_EXIT%\n'
                )
                system32 = Path(os.environ.get("SystemRoot", "C:\\Windows")) / "System32"
                path = str(bindir) + os.pathsep + str(system32)
            else:
                fake_bash.write_text(
                    "#!/usr/bin/env python3\n"
                    "import json, os, sys\n"
                    "from pathlib import Path\n"
                    "Path(os.environ['CAPTURED_ARGS']).write_text(json.dumps(sys.argv[1:]))\n"
                    "raise SystemExit(int(os.environ['FAKE_EXIT']))\n"
                )
                fake_bash.chmod(0o755)
                path = str(bindir) + os.pathsep + os.environ["PATH"]

            environment = dict(
                os.environ,
                PATH=path,
                CAPTURED_ARGS=str(captured),
                FAKE_EXIT="37",
            )
            arguments = [
                "fixture-image",
                "--crate",
                "tinysearch",
                "--version",
                "0.4.4",
            ]
            result = subprocess.run(
                [pwsh, "-NoProfile", "-File", str(script), *arguments],
                env=environment,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 37, result.stdout + result.stderr)
            if os.name == "nt":
                self.assertEqual(
                    captured.read_text().strip(),
                    f"{script.with_name('test-distro.sh')} {' '.join(arguments)}",
                )
            else:
                self.assertEqual(
                    json.loads(captured.read_text()),
                    [str(script.with_name("test-distro.sh")), *arguments],
                )

if __name__ == "__main__": unittest.main()
