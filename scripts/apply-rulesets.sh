#!/bin/sh
# Applies .github/rulesets/*.json to this repository on GitHub: creates each ruleset, or updates
# the one with the same name. Needs gh, logged in as an admin of the repository. GitHub allows
# rulesets on public repositories, and on private ones only with GitHub Pro.
#
#   protect main          no force-push to or deletion of the default branch
#   protect release tags  v* tags cannot be deleted or moved once pushed
set -eu
cd "$(dirname "$0")/.."
repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
for f in .github/rulesets/*.json; do
  name="$(sed -n 's/^ *"name": *"\([^"]*\)".*/\1/p' "$f")"
  id="$(gh api "repos/$repo/rulesets" --jq ".[] | select(.name == \"$name\") | .id")"
  if [ -n "$id" ]; then
    gh api -X PUT "repos/$repo/rulesets/$id" --input "$f" >/dev/null
    echo "updated '$name' on $repo"
  else
    gh api -X POST "repos/$repo/rulesets" --input "$f" >/dev/null
    echo "created '$name' on $repo"
  fi
done
