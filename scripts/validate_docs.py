#!/usr/bin/env python3
"""Validate tracked Markdown links/fences and tutorial/config payload syntax."""
import json
import math
from pathlib import Path
import re
import subprocess
try:
    import tomllib
except ImportError:
    raise SystemExit("Documentation validation requires Python 3.11 or later (for tomllib).")
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parents[1]


def tracked(pattern):
    result = subprocess.run(['git', 'ls-files', '-z', '--', pattern], cwd=ROOT,
                            capture_output=True, check=True)
    return [ROOT / path.decode() for path in result.stdout.split(b'\0') if path]


def strict_json(text):
    def object_pairs(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError(f'duplicate JSON key: {key}')
            value[key] = item
        return value
    def finite(value):
        number = float(value)
        if not math.isfinite(number):
            raise ValueError("nonfinite JSON")
        return number
    return json.loads(text, object_pairs_hook=object_pairs, parse_float=finite,
                      parse_constant=lambda _value: (_ for _ in ()).throw(ValueError('nonfinite JSON')))


def main():
    documents = tracked('*.md')
    for path in documents:
        text = path.read_text()
        fence = None
        body = []
        for number, line in enumerate(text.splitlines(), 1):
            if line.startswith('```'):
                if fence is None:
                    fence, body = line[3:].strip(), []
                else:
                    if fence == 'json':
                        strict_json('\n'.join(body))
                    elif fence == 'toml':
                        tomllib.loads('\n'.join(body))
                    fence = None
            elif fence is not None:
                body.append(line)
            else:
                for target in re.findall(r'!?\[[^\]]*\]\(([^)]+)\)', line):
                    target = target.strip('<>')
                    if urlparse(target).scheme or target.startswith('#'):
                        continue
                    destination = unquote(target.split('#', 1)[0])
                    if destination and not (path.parent / destination).exists():
                        raise ValueError(f'{path.relative_to(ROOT)}:{number}: missing link {target}')
        if fence is not None:
            raise ValueError(f'{path.relative_to(ROOT)}: unclosed code fence')
    payloads = tracked('fixtures/tutorial/*.json')
    for path in payloads:
        strict_json(path.read_text())
    configurations = tracked('fixtures/config/*.toml')
    for path in configurations:
        tomllib.loads(path.read_text())
    print(f'PASS: {len(documents)} tracked Markdown files, {len(payloads)} tutorial JSON files, {len(configurations)} configuration fixtures')


if __name__ == '__main__':
    main()
