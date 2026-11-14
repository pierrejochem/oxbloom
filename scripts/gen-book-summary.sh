#!/usr/bin/env bash
# Generates docs/SUMMARY.md for the mdBook developer site from the files under docs/.
# Run from the repository root. The generated file is not committed.
set -euo pipefail

out=docs/SUMMARY.md
title() { sed -n 's/^# //p;T;q' "$1" | head -n 1; }

{
  echo "# Summary"
  echo
  echo "[Introduction](introduction.md)"
  echo
  echo "# Guide"
  echo
  for f in docs/guide/*.md; do
    t=$(title "$f"); echo "- [${t:-$(basename "$f" .md)}](${f#docs/})"
  done
  echo
  echo "# Reference"
  echo
  echo "- [API reference](API.md)"
  echo
  echo "# Roadmaps"
  echo
  echo "- [Core features](ROADMAP-core.md)"
  echo "- [Modules](ROADMAP-modules.md)"
  echo
  echo "# Design specs"
  echo
  for f in docs/superpowers/specs/*.md; do
    t=$(title "$f"); echo "- [${t:-$(basename "$f" .md)}](${f#docs/})"
  done
  echo
  echo "# Implementation plans"
  echo
  for f in docs/superpowers/plans/*.md; do
    case "$f" in *.progress.md) continue ;; esac
    t=$(title "$f"); echo "- [${t:-$(basename "$f" .md)}](${f#docs/})"
  done
} > "$out"
