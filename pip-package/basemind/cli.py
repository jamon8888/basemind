"""CLI entry point for basemind."""

import os
import sys

from .downloader import run_basemind


def main() -> None:
    """Main entry point for the CLI (full profile by default)."""
    args = sys.argv[1:]
    run_basemind(args)


def main_legal() -> None:
    """Explicit command name for the confidential `hacienda-legal` profile.

    Sets the profile before the downloader resolves its asset, so the alias can
    never run a previously cached full-profile binary by accident.
    """
    os.environ["BASEMIND_PROFILE"] = "hacienda-legal"
    main()


if __name__ == "__main__":
    main()
