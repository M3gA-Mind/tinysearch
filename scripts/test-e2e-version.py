"""Pin module version selection with reordered workspace metadata."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest
import zipfile

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
        self.assertIn("$env:MSYS = 'noglob'", text)
        self.assertIn('$env:MSYS = $previousMsys', text)
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
            try:
                result = subprocess.run(
                    [pwsh, "-NoProfile", "-File", str(script), *arguments],
                    env=environment,
                    text=True,
                    capture_output=True,
                    timeout=30,
                )
            except subprocess.TimeoutExpired as error:
                self.fail(f"PowerShell distro wrapper timed out: {error}")
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

    def test_distro_powershell_rejects_unsupported_image_before_docker(self):
        """A real Windows call preserves the Bash validation exit and diagnostic."""
        if os.name != "nt":
            self.skipTest("requires the Windows PowerShell-to-Bash process boundary")

        pwsh = shutil.which("pwsh")
        if pwsh is None:
            self.fail("Windows module CI must provide pwsh for the distro wrapper test")

        script = Path(__file__).with_name("test-distro.ps1").resolve()
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            docker_called = fixture / "docker-called.txt"
            bash_env = fixture / "bash-env.sh"
            bash_env.write_text(
                'docker() { printf called > "$DOCKER_CALLED"; return 99; }\n'
            )
            environment = dict(
                os.environ,
                BASH_ENV=bash_env.as_posix(),
                DOCKER_CALLED=docker_called.as_posix(),
            )
            result = subprocess.run(
                [
                    pwsh,
                    "-NoProfile",
                    "-File",
                    str(script),
                    "unsupported image:latest",
                    "--crate",
                    "tinysearch",
                    "--version",
                    "0.4.4",
                    "--target",
                    "x86_64-unknown-linux-gnu",
                    "--host",
                    "ubuntu-22.04-x86_64",
                    "--extra-files-json",
                    "[]",
                    "--output-dir",
                    "target/test-distro-unsupported-image",
                ],
                env=environment,
                text=True,
                capture_output=True,
                timeout=30,
            )

        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("unsupported module build image: unsupported image:latest", result.stderr)
        self.assertFalse(docker_called.exists(), "Docker must not run for an unsupported image")

    def test_verifier_captures_the_existing_module_environment_before_try(self):
        """The finally block must preserve caller state even on early failures."""
        verifier_path = Path(__file__).with_name("verify-module.ps1").resolve()
        verifier = verifier_path.read_text()
        assignment = verifier.index("$previousArtifact = $env:TINYSEARCH_TEST_MODULE")
        try_block = verifier.index("try {")
        self.assertLess(assignment, try_block)
        pwsh = shutil.which("pwsh")
        if pwsh is None:
            return

        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "unsafe.zip"
            with zipfile.ZipFile(archive, "w") as package:
                package.writestr("../outside.dll", b"unsafe archive member")
            command = (
                f"try {{ & '{verifier_path}' '{archive}' }} catch {{ }}; "
                "[Console]::WriteLine($env:TINYSEARCH_TEST_MODULE)"
            )
            environment = dict(os.environ, TINYSEARCH_TEST_MODULE="caller-value")
            result = subprocess.run(
                [pwsh, "-NoProfile", "-Command", command],
                env=environment,
                text=True,
                capture_output=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout.strip(), "caller-value")

if __name__ == "__main__": unittest.main()
