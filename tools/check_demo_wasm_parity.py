#!/usr/bin/env python3
"""Regression test: build the real WASM engine against the pinned public synthetic fixture."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip()
    asset = repo / 'crates/oa-demo-scenario/tests/data/ord-normal-push-v1.oasd'
    digest = hashlib.sha256(asset.read_bytes()).hexdigest()
    if digest != '487157be621d883bb381fc30e809e1e1a507cf92f98f3b01606831554f8a216f':
        raise RuntimeError('reviewed synthetic scenario fixture digest changed')
    args.out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, OA_DEMO_SCENARIO_ASSET=str(asset),
               OA_DEMO_SCENARIO_ASSET_SHA256=digest, OA_F2_SOURCE_COMMIT=head)
    selected = {}
    for package, target, extra in (
        ('oa-demo-engine', 'oa-demo-runner', ['--bin', 'oa-demo-runner']),
        ('oa-demo-wasm', 'oa_demo_wasm', ['--target', 'wasm32-unknown-unknown']),
    ):
        command = ['cargo', '+1.89.0', 'build', '--release', '--locked', '--offline',
                   '-p', package, *extra, '--message-format=json-render-diagnostics']
        build = subprocess.run(command, cwd=repo, env=env, check=True,
                               capture_output=True, text=True, timeout=1200)
        (args.out / (package + '.cargo.jsonl')).write_text(build.stdout)
        (args.out / (package + '.cargo.stderr')).write_text(build.stderr)
        candidates = []
        for line in build.stdout.splitlines():
            row = json.loads(line)
            if row.get('reason') != 'compiler-artifact' or row['target']['name'] != target:
                continue
            if package == 'oa-demo-wasm':
                candidates.extend(Path(p) for p in row['filenames'] if p.endswith('.wasm'))
            elif row.get('executable'):
                candidates.append(Path(row['executable']))
        if len(candidates) != 1:
            raise RuntimeError(f'{package}: expected one exact Cargo artifact')
        selected[package] = candidates[0].resolve(strict=True)
    subprocess.run(['node', str(repo / 'tools/oa-demo-wasm/check_parity.mjs'),
                    str(selected['oa-demo-wasm']), str(selected['oa-demo-engine']), str(asset),
                    head, str(args.out.resolve())], cwd=repo, env=env, check=True, timeout=180)


if __name__ == '__main__':
    main()
