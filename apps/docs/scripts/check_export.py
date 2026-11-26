#!/usr/bin/env python3
"""Check the static Pages routes, fragments, assets, sitemap and ZBSearch records."""

import json
import os
import re
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit
from xml.etree import ElementTree

APP = Path(__file__).resolve().parents[1]
OUT = APP / "out"
BASE = os.environ.get("NEXT_PUBLIC_BASE_PATH", "/cairn-memory")
ORIGIN = os.environ.get("NEXT_PUBLIC_SITE_ORIGIN", "https://cunilab.github.io")
SITE = ORIGIN + BASE


class Page(HTMLParser):
    def __init__(self, path):
        super().__init__()
        self.ids = set()
        self.links = set()
        self.feed(path.read_text(encoding="utf-8"))

    def handle_starttag(self, tag, attrs):
        for attr, value in attrs:
            if attr == "id" and value:
                self.ids.add(value)
            if attr in {"href", "src", "poster", "xlink:href"} and value:
                self.links.add(value)
            if attr == "srcset" and value and not value.startswith("data:"):
                self.links.update(part.strip().split()[0] for part in value.split(",") if part.strip())

    handle_startendtag = handle_starttag


def route(path):
    if path.name == "index.html":
        return "/" + "/".join(path.parts[:-1]) + "/" if len(path.parts) > 1 else "/"
    return "/" + path.as_posix()


def expected_routes():
    urls = set()
    for article in (APP / "content").rglob("*"):
        if article.suffix not in {".md", ".mdx"}:
            continue
        path = article.relative_to(APP / "content").with_suffix("")
        segments = path.parts[:-1] if path.name == "index" else path.parts
        urls.add("/docs/" + "/".join(segments) + "/" if segments else "/")
    return urls


def main():
    pages = {route(path.relative_to(OUT)): Page(path) for path in OUT.rglob("*.html")}
    errors = set()

    def check(href, source):
        target = urlsplit(urljoin(SITE + source, href))
        if target.scheme not in {"http", "https"} or target.netloc != urlsplit(ORIGIN).netloc:
            return
        if not target.path.startswith(BASE + "/"):
            errors.add(f"{source}: outside Pages base path: {href}")
            return
        path = unquote(target.path[len(BASE):])
        page = pages.get(path) or pages.get(path + "/")
        if page:
            if target.fragment and unquote(target.fragment) not in page.ids:
                errors.add(f"{source}: missing fragment: {href}")
        elif not (OUT / path.lstrip("/")).is_file():
            errors.add(f"{source}: missing route or asset: {href}")

    expected = expected_routes()
    articles = {path for path in pages if path == "/" or path.startswith("/docs/")}
    if not expected or expected != articles:
        errors.add(f"Exported article routes differ from content: {sorted(expected ^ articles)}")

    for path, page in pages.items():
        for link in page.links:
            check(link, path)

    for css in OUT.rglob("*.css"):
        path = "/" + css.relative_to(OUT).as_posix()
        for asset in re.findall(r"""url\(\s*['"]?([^'")]+)""", css.read_text(encoding="utf-8")):
            check(asset.strip(), path)

    sitemap = ElementTree.parse(OUT / "sitemap.xml")
    locations = {element.text for element in sitemap.findall(".//{*}loc")}
    if locations != {SITE + path for path in expected}:
        errors.add("Sitemap URLs differ from the expected prefixed article routes")

    index = json.loads((OUT / "search-index.json").read_text(encoding="utf-8"))
    # These are ZBSearch's serialized records. Next Link adds the base path in the dialog.
    records = list(index["docs"]["docs"].values())
    indexed = {record["url"].rstrip("/") + "/" for record in records if record["type"] == "page"}
    if not records or indexed != expected:
        errors.add("The search index must include a page result for every article")
    for record in records:
        url = record["url"]
        if not url.startswith("/") or url.startswith("//"):
            errors.add(f"Search result must be a site route: {url}")
            continue
        check(BASE + url, "/search-index.json")

    if errors:
        raise SystemExit("\n".join(sorted(errors)))
    print(f"Checked {len(articles)} articles, internal links and assets, sitemap, and {len(records)} search records.")


if __name__ == "__main__":
    main()
