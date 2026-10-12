"""Verify the shipped module archive layout without compiling native code."""
import hashlib
import io
import json
import os
import shlex
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parent.parent

class PackageTests(unittest.TestCase):
    def test_platform_verifiers_run_loaded_module_e2e_suite(self):
        unix_verifier = (ROOT / "scripts/verify-module.sh").read_text()
        windows_verifier = (ROOT / "scripts/verify-module.ps1").read_text()
        unix_e2e = (ROOT / "scripts/test-e2e.sh").read_text()
        windows_e2e_path = ROOT / "scripts/test-e2e.ps1"
        self.assertIn("--test module_e2e", unix_verifier)
        self.assertIn("--test module_e2e", windows_verifier)
        self.assertIn("verify-module.sh", unix_e2e)
        self.assertIn("scripts/test-e2e.ps1", unix_e2e)
        self.assertTrue(windows_e2e_path.is_file())
        windows_e2e = windows_e2e_path.read_text()
        self.assertIn("verify-module.ps1", windows_e2e)
        rustc_status = windows_e2e.index("if ($LASTEXITCODE -ne 0) { throw 'rustc -vV failed' }")
        host_parse = windows_e2e.index("$target = $hostLine.ToString()")
        self.assertLess(rustc_status, host_parse)
        self.assertIn("if (-not $hostLine) { throw 'rustc host triple not found' }", windows_e2e)

    def test_windows_verifier_saves_the_existing_artifact_before_try(self):
        verifier = (ROOT / "scripts/verify-module.ps1").read_text()
        assignment = verifier.index("$previousArtifact = $env:TINYSEARCH_TEST_MODULE")
        try_block = verifier.index("try {")
        self.assertLess(assignment, try_block)

    def test_verify_extracts_artifact_under_the_current_user_home(self):
        target = ROOT / "target"; target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            fixture = Path(directory); home = fixture/"home"; home.mkdir()
            bindir = fixture/"bin"; bindir.mkdir()
            uname = bindir/"uname"
            uname.write_text("#!/usr/bin/env bash\nprintf '%s\\n' Darwin\n")
            uname.chmod(0o755)
            archive = fixture/"module.tar.gz"
            with tarfile.open(archive, "w:gz") as package:
                for name, data in [("modules.toml", b""), ("libtinysearch.dylib", b"module")]:
                    member = tarfile.TarInfo(name); member.size = len(data)
                    package.addfile(member, io.BytesIO(data))
            cargo = bindir/"cargo"
            cargo.write_text("#!/usr/bin/env python3\nimport os,sys\nfrom pathlib import Path\n"
                "module = os.environ.get('TINYSEARCH_TEST_MODULE') or os.environ.get('TINYJUICE_TEST_MODULE') or os.environ.get('TINYCHANNELS_MODULE_ARTIFACT') or sys.argv[-1]\n"
                "Path(os.environ['RECORDED_MODULE_PATH']).write_text(module)\n"
                "Path(os.environ['RECORDED_MODULE_PATH']+'.exists').write_text(str(Path(module).is_file()))\n")
            cargo.chmod(0o755)
            env = dict(os.environ, HOME=str(home), PATH=str(bindir)+os.pathsep+os.environ["PATH"],
                RECORDED_MODULE_PATH=str(fixture/"loaded-module-path"))
            env.pop("CARGO_TARGET_DIR", None)
            result = subprocess.run(["bash", str(ROOT/"scripts/verify-module.sh"),
                "--archive", str(archive)], cwd=ROOT, env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            module_path = Path((fixture/"loaded-module-path").read_text())
            self.assertTrue(module_path.is_relative_to(home/".cache"), module_path)
            self.assertEqual(Path(str(fixture/"loaded-module-path")+".exists").read_text(), "True")

    def test_verifier_rejects_symlinked_home_path_components(self):
        target = ROOT / "target"; target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            fixture = Path(directory)
            real_parent = fixture/"real-parent"; real_parent.mkdir()
            (real_parent/"home").mkdir()
            linked_parent = fixture/"linked-parent"
            linked_parent.symlink_to(real_parent, target_is_directory=True)
            home = linked_parent/"home"
            bindir = fixture/"bin"; bindir.mkdir()
            (bindir/"uname").write_text("\n".join(["#!/usr/bin/env bash", "echo Darwin"])+"\n")
            (bindir/"uname").chmod(0o755)
            archive = fixture/"module.tar.gz"
            with tarfile.open(archive, "w:gz") as package:
                for name, data in [("modules.toml", b""), ("libtinysearch.dylib", b"module")]:
                    member = tarfile.TarInfo(name); member.size = len(data)
                    package.addfile(member, io.BytesIO(data))
            cargo_called = fixture/"cargo-called"
            cargo = bindir/"cargo"
            cargo.write_text("\n".join(["#!/usr/bin/env python3", "import os",
                "from pathlib import Path", "Path(os.environ['CARGO_WAS_CALLED']).touch()"])+"\n")
            cargo.chmod(0o755)
            env = dict(os.environ, HOME=str(home), PATH=str(bindir)+os.pathsep+os.environ["PATH"],
                CARGO_WAS_CALLED=str(cargo_called))
            env.pop("CARGO_TARGET_DIR", None)
            result = subprocess.run(["bash", str(ROOT/"scripts/verify-module.sh"),
                "--archive", str(archive)], cwd=ROOT, env=env, text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertFalse(cargo_called.exists())

    def test_verifier_rejects_writable_cache_without_sticky_bit(self):
        target = ROOT / "target"; target.mkdir(exist_ok=True)
        for mode, should_succeed in [(0o777, False), (0o770, False), (0o1777, True)]:
            with self.subTest(mode=oct(mode)), tempfile.TemporaryDirectory(dir=target) as directory:
                fixture = Path(directory); home = fixture/"home"; home.mkdir()
                cache = home/".cache"; cache.mkdir(); cache.chmod(mode)
                bindir = fixture/"bin"; bindir.mkdir()
                uname = bindir/"uname"
                uname.write_text("#!/usr/bin/env bash\nprintf '%s\\n' Darwin\n")
                uname.chmod(0o755)
                archive = fixture/"module.tar.gz"
                with tarfile.open(archive, "w:gz") as package:
                    for name, data in [("modules.toml", b""), ("libtinysearch.dylib", b"module")]:
                        member = tarfile.TarInfo(name); member.size = len(data)
                        package.addfile(member, io.BytesIO(data))
                cargo_called = fixture/"cargo-called"
                cargo = bindir/"cargo"
                cargo.write_text("#!/usr/bin/env python3\nimport os\nfrom pathlib import Path\n"
                    "Path(os.environ['CARGO_WAS_CALLED']).touch()\n")
                cargo.chmod(0o755)
                env = dict(os.environ, HOME=str(home), PATH=str(bindir)+os.pathsep+os.environ["PATH"],
                    CARGO_WAS_CALLED=str(cargo_called))
                env.pop("CARGO_TARGET_DIR", None)
                result = subprocess.run(["bash", str(ROOT/"scripts/verify-module.sh"),
                    "--archive", str(archive)], cwd=ROOT, env=env, text=True, capture_output=True)
                if should_succeed:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertTrue(cargo_called.exists())
                else:
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertFalse(cargo_called.exists())

    def test_arch_package_install_disables_only_landlock_sandbox(self):
        script = (ROOT / "scripts/test-distro.sh").read_text()
        pacman = next(line.strip() for line in script.splitlines() if line.strip().startswith("pacman "))
        self.assertEqual(shlex.split(pacman)[:3], ["pacman", "--disable-sandbox-filesystem", "-Syu"])
        self.assertNotIn("--disable-sandbox", shlex.split(pacman))

    def test_archive_attests_host_library_bytes_from_a_symlinked_checkout(self):
        """Package the actual build script and verify the shipped bytes and attestation."""
        target = ROOT / "target"; target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            root = Path(directory); (root/"scripts").mkdir(); (root/"bin").mkdir()
            shutil.copy2(ROOT/"scripts/build-module.sh", root/"scripts/build-module.sh")
            for file in ["LICENSE","README.md","MODULE.md"]: (root/file).write_text("fixture")
            cargo = root/"bin/cargo"
            cargo.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                "root=Path.cwd()\n"
                "if 'metadata' in sys.argv: print(json.dumps({'target_directory':str(root/'target')}))\n"
                "else:\n"
                " target=sys.argv[sys.argv.index('--target')+1]\n"
                " output=root/'target'/target/'release'; output.mkdir(parents=True,exist_ok=True)\n"
                " (output/'libtinysearch.so').write_bytes(b'compiled-fixture')\n")
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=str(root/"bin")+os.pathsep+os.environ["PATH"])
            env.pop("CARGO_TARGET_DIR", None)
            alias = root/"checkout-link"; alias.symlink_to(root, target_is_directory=True)
            result = subprocess.run(["bash",str(alias/"scripts/build-module.sh"),
                "--crate","tinysearch","--version","1.2.3-beta.1",
                "--target","x86_64-unknown-linux-gnu","--host","ubuntu-22.04-x86_64"],
                cwd=root,env=env,text=True,capture_output=True)
            self.assertEqual(result.returncode,0,result.stderr)
            archive=root/"release-assets/tinysearch-1.2.3-beta.1-ubuntu-22.04-x86_64.tar.gz"
            with tarfile.open(archive) as package:
                data=package.extractfile("./libtinysearch.so").read()
                manifest=tomllib.loads(package.extractfile("./modules.toml").read().decode())
                self.assertEqual(data,b"compiled-fixture")
                self.assertEqual(manifest,{"libtinysearch.so":hashlib.sha256(data).hexdigest()})
                for name in ["./LICENSE","./README.md","./MODULE.md"]:
                    self.assertIsNotNone(package.extractfile(name))

if __name__ == "__main__": unittest.main()
