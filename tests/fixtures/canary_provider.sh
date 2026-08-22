#!/bin/sh
# Canary command provider for the Phase-3 injection-plan tests.
#
# Used as a credential `argv`: canary_provider.sh <canary-path> <value>
#
# The script creates <canary-path> the moment it runs, so a test proves by the
# file's ABSENCE that agentenv never resolved this provider — the canary
# half of the conflict criteria, which require the injection plan to reject a
# conflict before any provider is touched.
#
# <value> goes to stdout, which agentenv captures as the credential value.
# Nothing is written to stderr: a command provider's stderr is inherited, and a
# planted sentinel must never reach an inherited channel, so the suite-wide
# no-secret grep measures agentenv's own output only.
#
# Pairs with tests/fixtures/bin/probe.rs (the `test-probe` binary): the probe
# reports the child environment, this script reports whether resolution ran.
set -eu

: > "$1"
printf '%s\n' "$2"
