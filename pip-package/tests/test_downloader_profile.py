"""Wrapper tests for explicit profile selection and asset resolution.

The legacy `pip install basemind` command keeps installing `full`; the
confidential profile is selected by name (BASEMIND_PROFILE=hacienda-legal or
the `basemind-hacienda-legal` command) and must resolve its own asset, verify
its own SHA-256 entry, and fail explicitly when that asset is missing.
"""

from __future__ import annotations

import pytest
from basemind import cli, downloader


def _profile(monkeypatch, value: str | None) -> str:
    if value is None:
        monkeypatch.delenv("BASEMIND_PROFILE", raising=False)
    else:
        monkeypatch.setenv("BASEMIND_PROFILE", value)
    return downloader._profile()


def test_default_profile_is_full(monkeypatch):
    assert _profile(monkeypatch, None) == "full"


def test_explicit_hacienda_legal_profile(monkeypatch):
    assert _profile(monkeypatch, "hacienda-legal") == "hacienda-legal"


def test_unknown_profile_fails_before_download(monkeypatch):
    with pytest.raises(RuntimeError, match="unknown BASEMIND_PROFILE 'legal'"):
        _profile(monkeypatch, "legal")


def test_asset_prefix_keeps_full_name_and_prefixes_legal():
    assert downloader._asset_prefix("full") == "basemind"
    assert downloader._asset_prefix("hacienda-legal") == "basemind-hacienda-legal"


@pytest.mark.parametrize(
    ("profile", "expected"),
    [
        ("full", "basemind-x86_64-unknown-linux-gnu.tar.gz"),
        ("hacienda-legal", "basemind-hacienda-legal-x86_64-unknown-linux-gnu.tar.gz"),
    ],
)
def test_asset_selects_profile_and_triple(monkeypatch, profile, expected):
    monkeypatch.setattr(downloader, "_platform_triple", lambda: "x86_64-unknown-linux-gnu")
    archive_url, ext, asset_name, checksums_url = downloader._asset("0.39.0", profile)
    assert asset_name == expected
    assert ext == "tar.gz"
    assert archive_url.endswith(f"/v0.39.0/{expected}")
    assert checksums_url.endswith("/v0.39.0/basemind_0.39.0_checksums.txt")


def test_release_url_uses_the_canonical_goldziher_upstream(monkeypatch):
    """Goldziher is the canonical release source (upstream remote + product
    metadata); pin it so a regression to the fork's host is an explicit change.
    """
    monkeypatch.setattr(downloader, "_platform_triple", lambda: "x86_64-unknown-linux-gnu")
    archive_url, _, _, checksums_url = downloader._asset("0.39.0", "full")
    assert archive_url.startswith("https://github.com/Goldziher/basemind/releases/download/v0.39.0/")
    assert (
        checksums_url == "https://github.com/Goldziher/basemind/releases/download/v0.39.0/basemind_0.39.0_checksums.txt"
    )


def test_missing_asset_error_names_profile_triple_version_and_asset(monkeypatch):
    monkeypatch.setattr(downloader, "_platform_triple", lambda: "aarch64-apple-darwin")
    error = downloader._missing_asset_error(
        "release asset is missing",
        "0.39.0",
        "hacienda-legal",
        "basemind-hacienda-legal-aarch64-apple-darwin.tar.gz",
    )
    message = str(error)
    for fragment in (
        "hacienda-legal",
        "aarch64-apple-darwin",
        "0.39.0",
        "basemind-hacienda-legal-aarch64-apple-darwin.tar.gz",
        "installers (.exe/.dmg)",
    ):
        assert fragment in message


def test_checksums_file_distinguishes_profiles_at_one_version():
    sums = (
        "aaaa  basemind-x86_64-unknown-linux-gnu.tar.gz\n"
        "bbbb  basemind-hacienda-legal-x86_64-unknown-linux-gnu.tar.gz\n"
    )
    assert downloader._expected_digest(sums, "basemind-hacienda-legal-x86_64-unknown-linux-gnu.tar.gz") == "bbbb"
    assert downloader._expected_digest(sums, "basemind-x86_64-unknown-linux-gnu.tar.gz") == "aaaa"
    assert downloader._expected_digest(sums, "basemind-hacienda-legal-aarch64-apple-darwin.tar.gz") is None


def test_legal_command_name_selects_the_profile(monkeypatch):
    monkeypatch.delenv("BASEMIND_PROFILE", raising=False)
    monkeypatch.setattr(cli, "run_basemind", lambda args: args)
    monkeypatch.setattr("sys.argv", ["basemind-hacienda-legal", "--version"])
    cli.main_legal()
    assert downloader._profile() == "hacienda-legal"


def test_switching_profile_replaces_the_cached_binary(monkeypatch, tmp_path):
    """A cached full binary must not survive a switch to the legal profile."""
    monkeypatch.setattr(downloader.Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.delenv("BASEMIND_BINARY", raising=False)
    monkeypatch.setenv("BASEMIND_PROFILE", "full")

    from basemind import __version__

    cache = tmp_path / ".cache" / "basemind" / __version__
    cache.mkdir(parents=True)
    (cache / "basemind").write_bytes(b"full-profile-binary")
    (cache / ".profile").write_text("full", encoding="utf-8")

    monkeypatch.setattr(downloader, "_download", lambda url, dest: None)
    monkeypatch.setattr(downloader, "_verify_checksum", lambda *a, **k: None)

    def fake_extract(archive, ext, destination):
        (destination / "basemind").write_bytes(b"legal-profile-binary")

    monkeypatch.setattr(downloader, "_extract", fake_extract)
    monkeypatch.setenv("BASEMIND_PROFILE", "hacienda-legal")

    binary = downloader.ensure_binary()

    assert (cache / "basemind").read_bytes() == b"legal-profile-binary"
    assert (cache / ".profile").read_text(encoding="utf-8") == "hacienda-legal"
    assert binary == str(cache / "basemind")
