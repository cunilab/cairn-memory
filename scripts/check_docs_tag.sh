#!/usr/bin/env bash
# Check the current remote state, including on a rerun of a previous tag event.
set -euo pipefail

tag=${1:?usage: check_docs_tag.sh TAG CHECKED_OUT_SHA}
expected_commit=$(git rev-parse "${2:?missing checked-out SHA}^{commit}")
stable='v(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)'
if [[ ! $tag =~ ^$stable$ ]]; then
  echo "Docs require a stable vMAJOR.MINOR.PATCH tag: $tag" >&2
  exit 1
fi

latest=$(git ls-remote --tags --refs --sort=-version:refname origin 'refs/tags/v*.*.*' \
  | cut -f2 | sed 's|^refs/tags/||' | awk -v stable="^$stable$" '$0 ~ stable && !seen++')
if [[ $tag != "$latest" ]]; then
  echo "Refusing docs for $tag. Latest stable tag: ${latest:-none}" >&2
  exit 1
fi

git fetch --no-tags origin '+refs/heads/main:refs/remotes/origin/main' "+refs/tags/$tag:refs/tags/$tag"
commit=$(git rev-parse "refs/tags/$tag^{commit}")
if [[ $commit != "$expected_commit" ]] || ! git merge-base --is-ancestor "$commit" origin/main; then
  echo "Tag $tag must point to the checked-out commit on main" >&2
  exit 1
fi
