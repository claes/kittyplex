#!/usr/bin/env python3
"""Validate local documentation links, offline CLI help and the installed manual."""
import argparse
import gzip
import os
from pathlib import Path
import re
import subprocess


def anchors(path):
    result = set()
    counts = {}
    fenced = False
    for line in path.read_text().splitlines():
        if line.startswith(('```', '~~~')):
            fenced = not fenced
        if fenced:
            continue
        heading = re.match(r'^#{1,6}\s+(.+?)\s*#*$', line)
        if heading:
            slug = re.sub(r'[^\w\- ]', '', heading[1].lower()).replace(' ', '-')
            count = counts.get(slug, 0)
            counts[slug] = count + 1
            result.add(slug + (f'-{count}' if count else ''))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--lint-man', action='store_true')
    parser.add_argument('--installed-man', action='store_true')
    args = parser.parse_args()
    root = args.root.resolve()
    binary = (args.binary or root/'target/debug/kittyplex').resolve()
    documents = [root/'README.md', *sorted((root/'docs').glob('*.md'))]
    for document in documents:
        for target in re.findall(r'\]\(([^)]+)\)', document.read_text()):
            if '://' in target or target.startswith('mailto:'):
                continue
            name, _, fragment = target.partition('#')
            path = document.parent/name if name else document
            assert path.is_file(), (document.name, target)
            if fragment and path.suffix == '.md':
                assert fragment in anchors(path), (document.name, target, 'missing heading')
    manual = root/'docs/kittyplex.1'
    source = manual.read_text()
    version = subprocess.check_output([str(binary), '--version'], text=True).strip()
    assert version in source, 'manual version differs from executable'
    # Exercise help with an unusable controller socket and no controlling TTY.
    env = dict(os.environ, KITTYPLEX_SOCKET='/nonexistent/kittyplex-doc-check.sock')
    for command in ['', 'start', 'run0', 'ssh', 'ctl', 'doctor']:
        argv = [str(binary), *([command] if command else []), '--help']
        output = subprocess.check_output(argv, env=env, text=True, stdin=subprocess.DEVNULL)
        assert 'Usage:' in output, command
        assert 'kittyplex(1)' in output or 'man kittyplex' in output, command
        assert not any(name in output for name in ['__start', '__broker', '__view', '__launch-tab']), command
        for option in set(re.findall(r'--[a-z][a-z-]+', output)):
            assert option in source.replace('\\-', '-'), (command, option, 'missing from manual')
    if args.lint_man:
        subprocess.run(['mandoc', '-T', 'lint', str(manual)], check=True)
        rendered = subprocess.check_output(['mandoc', '-T', 'ascii', str(manual)], text=True)
        # Remove overstrike bold/underline when checking the rendered text.
        while '\b' in rendered:
            rendered = re.sub('.\x08', '', rendered)
        for text in ['SYNOPSIS', 'ENVIRONMENT', 'EXIT STATUS', 'kittyplex run0', 'KITTYPLEX_SOCKET']:
            assert text in rendered, (text, 'missing from rendered manual')
    if args.installed_man:
        pages = list((binary.parent.parent/'share/man/man1').glob('kittyplex.1*'))
        assert len(pages) == 1, ('missing/ambiguous installed manual', pages)
        installed = gzip.open(pages[0], 'rt').read() if pages[0].suffix == '.gz' else pages[0].read_text()
        assert installed == source, 'installed manual differs from source'
    print('Documentation links/anchors, command help and manual checks passed')


if __name__ == '__main__':
    main()
