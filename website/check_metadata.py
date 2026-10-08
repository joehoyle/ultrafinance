"""Validate production metadata and referenced assets without network access."""

import json
import struct
import xml.etree.ElementTree as ET
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlparse


class Metadata(HTMLParser):
    def __init__(self):
        super().__init__()
        self.meta = {}
        self.links = []
        self.schemas = []
        self.in_schema = False
        self.schema = ""

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "meta":
            key = attrs.get("property", attrs.get("name"))
            if key:
                assert key not in self.meta, f"Duplicate metadata: {key}"
                self.meta[key] = attrs.get("content", "")
        if tag == "link":
            self.links.append(attrs)
        if tag == "script" and attrs.get("type") == "application/ld+json":
            self.in_schema = True
            self.schema = ""

    def handle_data(self, text):
        if self.in_schema:
            self.schema += text

    def handle_endtag(self, tag):
        if tag == "script" and self.in_schema:
            self.schemas.append(json.loads(self.schema))
            self.in_schema = False


site = Path(__file__).resolve().parent
metadata = Metadata()
metadata.feed((site / "index.html").read_text())
canonical_links = [link for link in metadata.links if link.get("rel") == "canonical"]
assert len(canonical_links) == 1, "Exactly one canonical URL is required"
canonical = canonical_links[0]["href"]
assert canonical == "https://ultrafinance.app/"
for key in ["description", "robots", "og:type", "og:site_name", "og:url",
            "og:title", "og:description", "og:image", "og:image:type",
            "og:image:width", "og:image:height", "og:image:alt", "twitter:card",
            "twitter:title", "twitter:description", "twitter:image", "twitter:image:alt"]:
    assert metadata.meta.get(key), f"Missing {key}"
assert metadata.meta["og:url"] == canonical
assert metadata.meta["twitter:card"] == "summary_large_image"
assert metadata.meta["og:image"] == metadata.meta["twitter:image"]
image_url = urlparse(metadata.meta["og:image"])
assert image_url.scheme == "https" and image_url.netloc == urlparse(canonical).netloc
image = (site / image_url.path.lstrip("/")).read_bytes()
assert image[:8] == b"\x89PNG\r\n\x1a\n"
assert struct.unpack(">II", image[16:24]) == (
    int(metadata.meta["og:image:width"]), int(metadata.meta["og:image:height"])
) == (1200, 630), "Share image dimensions must match metadata"
for link in metadata.links:
    if link.get("rel") in ["icon", "apple-touch-icon"]:
        assert (site / link["href"].lstrip("/")).is_file(), f"Missing icon: {link['href']}"
root = ET.parse(site / "sitemap.xml").getroot()
assert [element.text for element in root.iter("{http://www.sitemaps.org/schemas/sitemap/0.9}loc")] == [canonical, canonical + "sources", canonical + "privacy", canonical + "terms"]
assert f"Sitemap: {canonical}sitemap.xml" in (site / "robots.txt").read_text()
assert metadata.schemas and metadata.schemas[0]["@context"] == "https://schema.org"
for entity in metadata.schemas[0]["@graph"]:
    assert entity["url"] == canonical
assert 'href="https://github.com/"' not in (site / "index.html").read_text()
sources = Metadata()
sources.feed((site / "sources.html").read_text())
assert [link["href"] for link in sources.links if link.get("rel") == "canonical"] == [canonical + "sources"]
assert sources.meta["og:url"] == canonical + "sources"
assert sources.meta["description"]
assert 'href="/sources"' in (site / "index.html").read_text()
for slug in ["privacy", "terms"]:
    page = Metadata()
    page.feed((site / f"{slug}.html").read_text())
    assert [link["href"] for link in page.links if link.get("rel") == "canonical"] == [canonical + slug]
    assert page.meta["og:url"] == canonical + slug
    assert page.meta["description"]
    for filename in ["index.html", "sources.html", "privacy.html", "terms.html"]:
        assert f'href="/{slug}"' in (site / filename).read_text()
print("Production metadata, structured data, sitemap, icons, and share image validated.")
