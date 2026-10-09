#!/usr/bin/env python3
"""Generate the bundled offline gazetteer from pinned public input files.

Pass --input-dir for reproducible offline regeneration. With --download, fetch
fresh inputs into that directory first. Output is deterministic for those inputs;
review the manifest diff before committing a refreshed snapshot.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import unicodedata
import urllib.request
import zipfile

INPUTS = {
    "cities500.zip": "https://download.geonames.org/export/dump/cities500.zip",
    "alternateNamesV2.zip": "https://download.geonames.org/export/dump/alternateNamesV2.zip",
    "admin1CodesASCII.txt": "https://download.geonames.org/export/dump/admin1CodesASCII.txt",
    "countryInfo.txt": "https://download.geonames.org/export/dump/countryInfo.txt",
    "ISO-LICENSE.txt": "https://raw.githubusercontent.com/pycountry/pycountry/main/LICENSE.txt",
    "iso3166-2.json": "https://raw.githubusercontent.com/pycountry/pycountry/main/src/pycountry/databases/iso3166-2.json",
}


def normalize(value):
    chars = unicodedata.normalize("NFKD", value)
    chars = "".join(c for c in chars if not unicodedata.combining(c)).lower()
    return " ".join("".join(c if c.isalnum() else " " for c in chars).split())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input-dir", type=Path, required=True)
    parser.add_argument("--download", action="store_true")
    parser.add_argument("--verify", action="store_true", help="Verify the pinned inputs and generated files without changing output")
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / "crates/ultrafinance-core/data/geonames")
    args = parser.parse_args()
    args.input_dir.mkdir(parents=True, exist_ok=True)
    if args.download:
        for name, url in INPUTS.items():
            temporary = args.input_dir / (name + ".download")
            with urllib.request.urlopen(url, timeout=180) as response, temporary.open("wb") as target:
                while chunk := response.read(1024 * 1024):
                    target.write(chunk)
            temporary.replace(args.input_dir / name)
    inputs = []
    for name, url in INPUTS.items():
        path = args.input_dir / name
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        inputs.append({"file": name, "url": url, "sha256": digest})
    countries = {}
    for line in (args.input_dir / "countryInfo.txt").read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        countries[fields[0]] = (fields[1], {language.split("-")[0] for language in fields[15].split(",")} | {"en"})
    subdivisions = json.loads((args.input_dir / "iso3166-2.json").read_text())["3166-2"]
    by_name = {}
    by_code = {}
    for subdivision in subdivisions:
        country, code = subdivision["code"].split("-", 1)
        by_name.setdefault((country, normalize(subdivision["name"])), []).append(subdivision)
        by_code[(country, code)] = subdivision
    # English GeoNames names versus Dutch ISO names; these are region bridges,
    # not manually selected cities. Unmapped regions retain their full name.
    translations = {"north holland": "noord holland", "south holland": "zuid holland", "north brabant": "noord brabant", "friesland": "fryslan"}
    regions = {}
    mapped = 0
    for line in (args.input_dir / "admin1CodesASCII.txt").read_text().splitlines():
        key, name, ascii_name, _ = line.split("\t")
        country, geocode = key.split(".", 1)
        candidates = by_name.get((country, normalize(name)), [])
        if country == "NL" and normalize(name) in translations:
            candidates = by_name.get((country, translations[normalize(name)]), [])
        if country in {"US", "CH", "BE", "ME"} and (country, geocode) in by_code:
            candidates = [by_code[(country, geocode)]]
        aliases = {normalize(name), normalize(ascii_name)}
        canonical = name
        if len(candidates) == 1:
            subdivision = candidates[0]
            canonical = subdivision["code"].split("-", 1)[1]
            aliases |= {normalize(canonical), normalize(subdivision["code"]), normalize(subdivision["name"])}
            mapped += 1
        regions[key] = (canonical, "|".join(sorted(aliases)))
    records = {}
    with zipfile.ZipFile(args.input_dir / "cities500.zip") as archive:
        date = "%04d-%02d-%02d" % archive.getinfo("cities500.txt").date_time[:3]
        with archive.open("cities500.txt") as data:
            for line in io.TextIOWrapper(data, encoding="utf-8"):
                fields = line.rstrip("\n").split("\t")
                if fields[6] != "P" or fields[8] not in countries:
                    continue
                identifier, name, ascii_name = fields[:3]
                country = fields[8]
                region, region_aliases = regions.get(country + "." + fields[10], ("", ""))
                records[identifier] = [identifier, name, country, countries[country][0], region, region_aliases, {normalize(name), normalize(ascii_name)}]
    with zipfile.ZipFile(args.input_dir / "alternateNamesV2.zip") as archive:
        with archive.open("alternateNamesV2.txt") as data:
            for line in io.TextIOWrapper(data, encoding="utf-8"):
                fields = line.rstrip("\n").split("\t")
                if len(fields) < 8 or fields[1] not in records:
                    continue
                record = records[fields[1]]
                # Preferred/short current names in English or the country's
                # official languages; exclude colloquial, historical and dated names.
                if fields[2] in countries[record[2]][1] and (fields[4] == "1" or fields[5] == "1") and fields[6] != "1" and fields[7] != "1" and not any(fields[8:]):
                    record[6].add(normalize(fields[3]))
    args.output.mkdir(parents=True, exist_ok=True)
    rows = []
    aliases = 0
    max_city_words = 0
    for record in sorted(records.values(), key=lambda row: int(row[0])):
        names = sorted(name for name in record[6] if name and "|" not in name)
        aliases += len(names)
        max_city_words = max(max_city_words, *(len(name.split()) for name in names))
        row = record[:6] + ["|".join(names)]
        if any("\t" in field or "\n" in field for field in row):
            raise ValueError("invalid TSV field")
        rows.append("\t".join(row))
    payload = ("\n".join(rows) + "\n").encode()
    manifest = {
        "version": 1, "source": "GeoNames cities500", "source_url": "https://www.geonames.org/", "license": "CC-BY-4.0",
        "snapshot_date": date, "records": len(rows), "countries": len({record[2] for record in records.values()}),
        "aliases": aliases, "max_city_words": max_city_words, "mapped_regions": mapped, "unmapped_regions": len(regions) - mapped,
        "alias_policy": "Canonical/ASCII plus preferred or short current English and official-language names; exclude historical, colloquial and dated alternate names.",
        "region_policy": "Unique ISO subdivision name mapping; otherwise full GeoNames region name. Numeric GeoNames/FIPS codes are not statement aliases.",
        "subdivision_source": "pycountry / Debian iso-codes", "subdivision_license": "LGPL-2.1",
        "sha256": hashlib.sha256(payload).hexdigest(), "inputs": inputs,
    }
    manifest_text = json.dumps(manifest, indent=2, ensure_ascii=False) + "\n"
    if args.verify:
        if ((args.output / "cities.tsv").read_bytes() != payload
                or (args.output / "manifest.json").read_text() != manifest_text
                or (args.output / "ISO-LICENSE.txt").read_bytes() != (args.input_dir / "ISO-LICENSE.txt").read_bytes()):
            raise ValueError("pinned input or generated snapshot differs")
        print("Verified pinned inputs and deterministic gazetteer output.")
    else:
        (args.output / "cities.tsv").write_bytes(payload)
        (args.output / "manifest.json").write_text(manifest_text)
        (args.output / "ISO-LICENSE.txt").write_bytes((args.input_dir / "ISO-LICENSE.txt").read_bytes())
    print(json.dumps({key: manifest[key] for key in ["snapshot_date", "records", "countries", "aliases", "mapped_regions", "unmapped_regions"]}))


if __name__ == "__main__":
    main()
