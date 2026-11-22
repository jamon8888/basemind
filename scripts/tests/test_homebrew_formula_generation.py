"""The Homebrew generator keeps `basemind.rb` and emits the confidential
profile as a separate tap formula file, never two classes in one file.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "scripts" / "update-homebrew-formula.sh"
TRIPLES = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
)
STEMS = ("basemind", "basemind-hacienda-legal")


def _checksums(directory: Path, *, omit: str | None = None) -> Path:
    lines: list[str] = []
    for stem_index, stem in enumerate(STEMS):
        for triple_index, triple in enumerate(TRIPLES):
            asset = f"{stem}-{triple}.tar.gz"
            if asset == omit:
                continue
            digest = f"{stem_index}{triple_index}" * 32
            lines.append(f"{digest}  {asset}")
    path = directory / "checksums.txt"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def _run(checksums: Path, legal_out: Path | None = None) -> subprocess.CompletedProcess[str]:
    command = ["bash", str(GENERATOR), "0.39.0", str(checksums)]
    if legal_out is not None:
        command.append(str(legal_out))
    return subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)


def test_full_formula_stays_a_single_class_on_stdout(tmp_path: Path) -> None:
    checksums = _checksums(tmp_path)
    legal_out = tmp_path / "basemind-hacienda-legal.rb"

    result = _run(checksums, legal_out)

    assert result.returncode == 0, result.stderr
    assert result.stdout.count("class ") == 1
    assert "class Basemind < Formula" in result.stdout
    assert "BasemindHaciendaLegal" not in result.stdout
    assert "hacienda-legal" not in result.stdout
    # The pre-existing formula contract is unchanged.
    assert 'desc "Full AI context layer over MCP — code-map, document RAG, memory, web, git"' in result.stdout
    assert (
        'url "https://github.com/Goldziher/basemind/releases/download/v0.39.0/basemind-aarch64-apple-darwin.tar.gz"'
        in result.stdout
    )
    assert 'bin.install_symlink libexec/"basemind"' in result.stdout
    assert "assert_match version.to_s" in result.stdout


def test_legal_formula_is_written_to_its_own_file(tmp_path: Path) -> None:
    checksums = _checksums(tmp_path)
    legal_out = tmp_path / "basemind-hacienda-legal.rb"

    result = _run(checksums, legal_out)
    legal = legal_out.read_text(encoding="utf-8")

    assert result.returncode == 0, result.stderr
    assert legal.count("class ") == 1
    assert "class BasemindHaciendaLegal < Formula" in legal
    assert "Class Basemind < Formula" not in legal
    for triple in TRIPLES:
        asset = f"basemind-hacienda-legal-{triple}.tar.gz"
        assert f"/{asset}" in legal
    assert "macOS 15 (Sequoia) or newer" in legal


def test_default_legal_output_is_next_to_checksums_file(tmp_path: Path) -> None:
    checksums = _checksums(tmp_path)

    result = _run(checksums)

    assert result.returncode == 0, result.stderr
    assert (tmp_path / "basemind-hacienda-legal.rb").is_file()


def test_missing_legal_checksum_fails_before_writing_any_formula(tmp_path: Path) -> None:
    missing = "basemind-hacienda-legal-x86_64-apple-darwin.tar.gz"
    checksums = _checksums(tmp_path, omit=missing)
    legal_out = tmp_path / "basemind-hacienda-legal.rb"

    result = _run(checksums, legal_out)

    assert result.returncode != 0
    assert missing in result.stderr
    assert "refusing to generate a partial formula" in result.stderr
    assert not legal_out.exists()
    assert "class Basemind < Formula" not in result.stdout
