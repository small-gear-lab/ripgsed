"""Sphinx configuration.

English is the source language. Japanese is built from gettext catalogs in
locale/ja/LC_MESSAGES/*.po; see tools/build-docs.sh.
"""

project = "ripgsed"
author = "Small Gear Lab"
copyright = "2026, Small Gear Lab"

extensions = ["myst_parser"]
source_suffix = {".md": "markdown"}
myst_heading_anchors = 3

exclude_patterns = ["_build", "ja", "locale", "requirements.txt"]

language = "en"
locale_dirs = ["locale/"]
gettext_compact = False

html_theme = "alabaster"
