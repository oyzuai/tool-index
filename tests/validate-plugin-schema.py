"""Validate plugins.json files against the vendored canonical catalog schema.

Usage: validate-plugin-schema.py FILE...

Shape only (JSON Schema 2020-12, `catalog.schema.json` with its sibling
`common.schema.json`); the semantic rules are checked by the release tool
(`tool-index-release plugins check`) and its tests. Needs `jsonschema`.
"""
import json
import sys
from pathlib import Path

from jsonschema import Draft202012Validator
from referencing import Registry, Resource

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "plugins-v1alpha1"


def load(name):
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def main(paths):
    catalog = load("catalog.schema.json")
    common = load("common.schema.json")
    registry = Registry().with_resources(
        [(common["$id"], Resource.from_contents(common)), ("common.schema.json", Resource.from_contents(common))]
    )
    Draft202012Validator.check_schema(catalog)
    validator = Draft202012Validator(catalog, registry=registry)
    failed = False
    for path in paths:
        errors = sorted(validator.iter_errors(json.loads(Path(path).read_text(encoding="utf-8"))), key=str)
        for error in errors:
            failed = True
            print(f"{path}: {'/'.join(map(str, error.absolute_path))}: {error.message}", file=sys.stderr)
        if not errors:
            print(f"{path}: valid against catalog.schema.json")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
