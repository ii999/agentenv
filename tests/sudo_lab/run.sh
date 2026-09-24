#!/usr/bin/env bash
set -euo pipefail

readonly script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly repo_root="$(cd "${script_dir}/../.." && pwd)"
readonly image_name="agentenv-sudo-lab:s0"

output_dir="${repo_root}/.dev/artifacts/work/sudo-execution/scratch/sudo-lab"
if [[ ${1:-} == "--output-dir" && -n ${2:-} && $# -eq 2 ]]; then
    output_dir="$2"
elif [[ $# -ne 0 ]]; then
    printf 'usage: %s [--output-dir PATH]\n' "$0" >&2
    exit 2
fi
mkdir -p "${output_dir}"
readonly output_dir="$(cd "${output_dir}" && pwd)"

docker build --pull=false --tag "${image_name}" "${script_dir}"
docker run --rm \
    --network none \
    --volume "${output_dir}:/out" \
    "${image_name}" \
    --output /out/evidence.json
