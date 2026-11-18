#!/usr/bin/env bash
# Builds the Zensical site from docs/ and checks it. Needs Python 3.11+
# with requirements.txt installed (pip install -r requirements.txt).
set -euo pipefail
cd "$(dirname "$0")"
python3 scripts/stage.py
(cd build && zensical build --strict --clean)
python3 scripts/check_site.py
