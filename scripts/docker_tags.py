"""Select the project-version tag for the manually published Docker image."""
import json
import os
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def image_tag(version, web_version, ref_type, ref_name):
    if web_version != version:
        raise ValueError(f'Web version {web_version} does not match Rust version {version}')
    if not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}', version):
        raise ValueError(f'Project version is not a valid Docker tag: {version!r}')
    if ref_type == 'tag' and ref_name != f'v{version}':
        raise ValueError(f'Expected version tag v{version}')
    if ref_type not in ('tag', 'branch'):
        raise ValueError(f'Unsupported Git ref type: {ref_type!r}')
    return f'docker.io/erquren/ergent:{version}'


def main():
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
    web_version = json.loads((ROOT / 'apps/web/package.json').read_text(encoding='utf-8'))['version']
    tag = image_tag(version, web_version, os.environ['GITHUB_REF_TYPE'], os.environ['GITHUB_REF_NAME'])
    with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
        output.write(f'tags={tag}\nversion={version}\n')
    print(f'Docker image: {tag}')


if __name__ == '__main__':
    main()
