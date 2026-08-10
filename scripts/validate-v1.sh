#!/usr/bin/env bash
echo 'note: validate-v1.sh is a compatibility wrapper; Draft Generation 1 is not frozen.' >&2
exec "$(cd "$(dirname "$0")" && pwd)/validate-draft.sh" "$@"
