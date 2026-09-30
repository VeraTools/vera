#!/bin/bash
set -euo pipefail

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version>"
  echo "Example: $0 0.5.0"
  exit 1
fi

# Validate semver format
if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$'; then
  echo "Error: '$VERSION' is not a valid semver version"
  exit 1
fi

TAG="v$VERSION"

# Check for uncommitted changes
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Error: uncommitted changes. Commit or stash first."
  exit 1
fi

# Releases must use the exact tested, synchronized master commit.
if [[ "$(git branch --show-current)" != master ]]; then
  echo "Error: release from master."
  exit 1
fi
git fetch origin master --tags
COMMIT=$(git rev-parse HEAD)
REMOTE_COMMIT=$(git ls-remote --exit-code origin refs/heads/master | cut -f1)
if [[ "$COMMIT" != "$REMOTE_COMMIT" ]] || [[ "$COMMIT" != "$(git rev-parse origin/master)" ]]; then
  echo "Error: local master must match origin/master."
  exit 1
fi

if git show-ref --verify --quiet "refs/tags/$TAG"; then
  echo "Error: tag $TAG already exists"
  exit 1
fi
if git ls-remote --exit-code origin "refs/tags/$TAG" >/dev/null; then
  echo "Error: remote tag $TAG already exists"
  exit 1
elif [[ $? != 2 ]]; then
  echo "Error: could not check remote tags."
  exit 1
fi

CI_GREEN=$(gh run list --workflow ci.yml --event push --branch master \
  --commit "$COMMIT" --limit 1 --json status,conclusion \
  --jq '.[0] | .status == "completed" and .conclusion == "success"')
if [[ "$CI_GREEN" != true ]]; then
  echo "Error: CI must pass on master commit $COMMIT before tagging."
  exit 1
fi
if [[ "$COMMIT" != "$(git ls-remote --exit-code origin refs/heads/master | cut -f1)" ]]; then
  echo "Error: remote master changed during validation; retry from synchronized master."
  exit 1
fi

echo "Creating release $TAG..."
git tag "$TAG" "$COMMIT"
git push origin "refs/tags/$TAG"
echo "Done. Release workflow will start shortly."
echo "Commit: $COMMIT"
echo "Track it: gh run list --workflow release.yml --commit $COMMIT"
