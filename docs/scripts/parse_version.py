"""
Validate a docs deploy version and print its canonical form.

The canonical form is `X.Y.Z[-beta.N]`: a leading `v` is stripped and an
omitted patch defaults to 0. Exits non-zero (with the reason on stderr) if the
string does not parse, or does not match the kind demanded by `--require`.

Usage:
    python parse_version.py [--require {stable,prerelease}] <version>
"""

import argparse

from common import Version


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("version", help="version string to validate")
    parser.add_argument(
        "--require",
        choices=("stable", "prerelease"),
        default=None,
        help="additionally demand the version be of this kind",
    )
    args = parser.parse_args(argv)

    try:
        version = Version.from_str(args.version)
    except ValueError as err:
        parser.exit(1, f"{err} - refusing to deploy.\n")

    if args.require and (args.require == "prerelease") != version.is_prerelease:
        parser.exit(1, f"'{version}' is not a {args.require} version.\n")

    print(version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
