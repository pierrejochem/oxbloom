#!/usr/bin/env bash
# Generates docs/SUMMARY.md for the mdBook developer site from the files under docs/.
# Run from the repository root. The generated file is not committed.
set -euo pipefail

out=docs/SUMMARY.md

# Brand assets for the site: tokens, fonts, logos (copied, not committed).
rm -rf docs/assets
mkdir -p docs/assets
cp branding/tokens.css docs/assets/tokens.css
cp -r branding/fonts docs/assets/fonts
cp branding/logos/oxbloom-lockup.svg docs/assets/logo.svg
cp branding/logos/oxbloom-mark.svg docs/assets/mark.svg
cp branding/logos/oxbloom-mark-reversed.svg docs/assets/mark-reversed.svg
cp branding/logos/oxbloom-lockup-reversed.svg docs/assets/logo-reversed.svg
cp branding/logos/oxbloom-favicon.svg theme/favicon.svg
cp branding/logos/png/oxbloom-favicon-32.png theme/favicon.png
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
  echo "# Architecture decisions"
  echo
  echo "- [Architecture decision records](adr/README.md)"
  echo "  - [ADR template](adr/0000-template.md)"
  for f in docs/adr/[0-9][0-9][0-9][0-9]-*.md; do
    case "$f" in docs/adr/0000-*) continue ;; esac
    t=$(title "$f"); echo "  - [${t:-$(basename "$f" .md)}](${f#docs/})"
  done
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
