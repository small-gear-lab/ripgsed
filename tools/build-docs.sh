#!/usr/bin/env bash
# Build the documentation.
#
#   tools/build-docs.sh          build HTML (docs/_build/html/{en,ja}) and
#                                regenerate the Japanese Markdown (docs/ja, checked in)
#   tools/build-docs.sh update   refresh locale/ja/*.po from the English source first
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
docs="$root/docs"
out="$docs/_build"

run() { uv run --no-project --quiet --with-requirements "$docs/requirements.txt" "$@"; }

if [ "${1:-}" = "update" ]; then
  run sphinx-build -b gettext "$docs" "$out/gettext"
  run sphinx-intl update -p "$out/gettext" -l ja -d "$docs/locale"
fi

run sphinx-build -b html -d "$out/doctrees/en" "$docs" "$out/html/en"
run sphinx-build -b html -d "$out/doctrees/ja" -D language=ja "$docs" "$out/html/ja"
printf '<meta http-equiv="refresh" content="0; url=en/">\n' > "$out/html/index.html"

# Japanese Markdown, readable directly on GitHub.
rm -rf "$out/md"
run sphinx-build -b markdown -d "$out/doctrees/md" -D language=ja \
  -D extensions=myst_parser,sphinx_markdown_builder "$docs" "$out/md"
mkdir -p "$docs/ja"
cp "$out"/md/*.md "$docs/ja/"
