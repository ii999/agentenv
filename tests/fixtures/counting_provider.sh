#!/bin/sh
# Counting command provider for the Phase-3 injection-plan tests.
#
# Used as a credential `argv`: counting_provider.sh <counter-path> <value>
#
# The script appends one line to <counter-path> per execution, so a test counts
# resolutions exactly. Dedup rows require the count to be 1: a credential named
# by several references — identical effective pairs, or one credential under two
# target names — is resolved once and injected under each target name.
#
# <value> goes to stdout, which agentenv captures as the credential value.
# Nothing is written to stderr: a command provider's stderr is inherited, and a
# planted sentinel must never reach an inherited channel, so the suite-wide
# no-secret grep measures agentenv's own output only.
#
# Pairs with tests/fixtures/bin/probe.rs (the `test-probe` binary): the probe
# reports which names the child received, this script reports how many times the
# provider ran to supply them.
set -eu

printf 'invoked\n' >> "$1"
printf '%s\n' "$2"
