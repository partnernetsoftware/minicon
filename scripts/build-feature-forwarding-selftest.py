#!/usr/bin/env python3
"""Execute build.sh against recording commands; no compilation or cleanup effects."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    parent = root / "target"
    parent.mkdir(exist_ok=True)
    fixture = Path(tempfile.mkdtemp(prefix="build-feature-selftest-", dir=parent)).resolve()
    assert fixture.is_relative_to(parent.resolve())
    script_dir = fixture / "scripts"
    bin_dir = fixture / "bin"
    script_dir.mkdir()
    bin_dir.mkdir()
    script = script_dir / "build.sh"
    script.write_bytes((root / "scripts/build.sh").read_bytes())
    python_stub = bin_dir / "python3"
    python_stub.write_text("#!/bin/bash\nexit 0\n", encoding="utf-8")
    cargo_stub = bin_dir / "cargo"
    cargo_stub.write_text('#!/bin/bash\nprintf "%s\\n" CALL "$@" END >> calls.log\n', encoding="utf-8")
    for stub in (python_stub, cargo_stub):
        stub.chmod(0o755)
    git_bash = Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe"
    bash = str(git_bash) if os.name == "nt" and git_bash.is_file() else shutil.which("bash")
    assert bash, "Bash required"
    env = dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ["PATH"])
    try:
        cases = [
            ([], []),
            (["--features", "experimental-rust-console", "rust_console_", "--", "--exact", "--features"],
             ["rust_console_", "--", "--exact", "--features"]),
            (["--features=one", "-Ftwo", "-F", "three", "rust_console_", "--", "--exact"],
             ["rust_console_", "--", "--exact"]),
        ]
        for arguments, forwarded in cases:
            log = fixture / "calls.log"
            if log.exists():
                log.unlink()
            result = subprocess.run([bash, str(script), "test", *arguments], cwd=fixture,
                                    env=env, capture_output=True, text=True, timeout=30)
            assert result.returncode == 0, result.stderr
            calls = [part.strip().splitlines() for part in log.read_text().split("CALL\n")[1:]]
            assert len(calls) == 3, calls
            workspace, terminal, lexer = [call[:-1] for call in calls]
            if arguments:
                assert workspace[-len(arguments):] == arguments, workspace
            lint_config='build.rustflags=["-D","dead_code","-D","unused_variables","-D","unused_must_use"]'
            for name, fork in [("vt100", terminal), ("vte", lexer)]:
                assert fork == ["test", "--locked", "--manifest-path", f"third_party/{name}/Cargo.toml",
                                "--config", lint_config, "--lib", *forwarded], fork
            assert "build.rustflags=[\"-D\",\"dead_code\",\"-D\",\"unused_variables\",\"-D\",\"unused_must_use\"]" in workspace
        print("build feature forwarding: 3 black-box cases passed")
    finally:
        # Explicit owned files/directories only; never recursively remove paths.
        for path in [fixture / "calls.log", script, python_stub, cargo_stub]:
            path.unlink(missing_ok=True)
        for path in [fixture / "target", script_dir, bin_dir, fixture]:
            if path.exists():
                path.rmdir()


if __name__ == "__main__":
    main()
