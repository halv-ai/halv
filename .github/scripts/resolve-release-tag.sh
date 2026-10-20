#!/usr/bin/env bash
set -euo pipefail

repository="${1:?repository is required}"
tag_name="${2:?tag name is required}"

ref_json="$(gh api "repos/$repository/git/ref/tags/$tag_name")"
object_type="$(jq -r '.object.type' <<<"$ref_json")"
object_sha="$(jq -r '.object.sha' <<<"$ref_json")"

depth=0
while [[ "$object_type" == "tag" ]]; do
  depth=$((depth + 1))
  if ((depth > 16)); then
    echo "Release tag $tag_name has too many nested tag objects." >&2
    exit 1
  fi
  tag_json="$(gh api "repos/$repository/git/tags/$object_sha")"
  object_type="$(jq -r '.object.type' <<<"$tag_json")"
  object_sha="$(jq -r '.object.sha' <<<"$tag_json")"
done

if [[ "$object_type" != "commit" ]]; then
  echo "Release tag $tag_name does not resolve to a commit." >&2
  exit 1
fi

printf '%s\n' "$object_sha"
