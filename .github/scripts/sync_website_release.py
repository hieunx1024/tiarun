#!/usr/bin/env python3
"""Synchronize TiaRun static website release links, version, and sitemap dates."""

from __future__ import annotations

import argparse
import datetime as dt
import re
from pathlib import Path

REPO_RELEASES = "https://github.com/hieunx1024/tiarun/releases"
PACKAGE_FILENAMES = {
    "exe": "tiarun-setup.exe",
    "zip": "tiarun-windows-x86_64.zip",
    "deb": "tiarun_{version}_amd64.deb",
    "tar": "tiarun-linux-x86_64.tar.gz",
}


def format_mib(size_bytes: int, use_comma: bool) -> str:
    """Format byte size as MiB matching app.js Intl.NumberFormat output."""
    mib = f"{size_bytes / (1024 ** 2):.1f}"
    if use_comma:
        mib = mib.replace(".", ",")
    return f"{mib} MiB"


def collect_asset_sizes(assets_dir: Path | None, version: str) -> dict[str, int]:
    """Read byte sizes of release packages from an artifacts directory."""
    if assets_dir is None or not assets_dir.is_dir():
        return {}
    sizes: dict[str, int] = {}
    for pkg, template in PACKAGE_FILENAMES.items():
        filename = template.format(version=version)
        candidate = assets_dir / filename
        if candidate.is_file():
            sizes[pkg] = candidate.stat().st_size
    return sizes


def sync_html(html_path: Path, tag: str, version: str, sizes: dict[str, int]) -> bool:
    """Update release version, download links, and optional file sizes in an HTML page."""
    original = html_path.read_text(encoding="utf-8")
    updated = original
    use_comma = '<html lang="vi"' in updated

    updated = re.sub(
        r'("softwareVersion"\s*:\s*")[^"]+(")',
        rf"\g<1>{version}\g<2>",
        updated,
    )
    updated = re.sub(
        rf"{re.escape(REPO_RELEASES)}/tag/v[0-9A-Za-z._+-]+",
        f"{REPO_RELEASES}/tag/{tag}",
        updated,
    )
    updated = re.sub(
        r'(<span id="release-version">)[^<]*(</span>)',
        rf"\g<1>{tag}\g<2>",
        updated,
    )

    for pkg, template in PACKAGE_FILENAMES.items():
        filename = template.format(version=version)
        new_url = f"{REPO_RELEASES}/download/{tag}/{filename}"

        def replace_package_anchor(match: re.Match[str], url: str = new_url, pkg_key: str = pkg) -> str:
            anchor = match.group(0)
            anchor = re.sub(r'href="[^"]+"', f'href="{url}"', anchor, count=1)
            if pkg_key in sizes:
                size_text = format_mib(sizes[pkg_key], use_comma)
                anchor = re.sub(
                    r"<small(?:\s+hidden)?>[^<]*</small>",
                    f"<small>{size_text}</small>",
                    anchor,
                    count=1,
                )
            return anchor

        updated = re.sub(
            rf'<a\b[^>]*data-package="{pkg}"[^>]*>.*?</a>',
            replace_package_anchor,
            updated,
            flags=re.DOTALL,
        )

    if updated != original:
        html_path.write_text(updated, encoding="utf-8")
        return True
    return False


def sync_sitemap(sitemap_path: Path, release_date: str) -> bool:
    """Update <lastmod> dates in sitemap.xml."""
    if not sitemap_path.is_file():
        return False
    original = sitemap_path.read_text(encoding="utf-8")
    updated = re.sub(
        r"(<lastmod>)[^<]+(</lastmod>)",
        rf"\g<1>{release_date}\g<2>",
        original,
    )
    if updated != original:
        sitemap_path.write_text(updated, encoding="utf-8")
        return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True, help="Release tag, e.g. v1.1.0")
    parser.add_argument("--assets-dir", type=Path, default=None, help="Directory with built release assets")
    parser.add_argument("--website-dir", type=Path, default=Path("website"), help="Website root directory")
    parser.add_argument("--date", default=dt.date.today().isoformat(), help="ISO date for sitemap lastmod")
    args = parser.parse_args()

    tag = args.tag if args.tag.startswith("v") else f"v{args.tag}"
    version = tag.lstrip("v")
    sizes = collect_asset_sizes(args.assets_dir, version)

    changed: list[Path] = []
    for html_file in (args.website_dir / "index.html", args.website_dir / "vi" / "index.html"):
        if html_file.is_file() and sync_html(html_file, tag, version, sizes):
            changed.append(html_file)

    sitemap_file = args.website_dir / "sitemap.xml"
    if sync_sitemap(sitemap_file, args.date):
        changed.append(sitemap_file)

    print(f"Synced website to {tag} (updated {len(changed)} file(s))")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
