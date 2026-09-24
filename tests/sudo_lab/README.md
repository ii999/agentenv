# Sudo compatibility lab

This fixture creates disposable Linux users, sudoers policy, SSH host keys, and
synthetic passwords inside one container. SSH listens only on the container's
loopback interface, and `run.sh` starts the container with `--network none` and
no published ports. It does not inspect or modify host accounts, sudoers,
keychains, SSH configuration, or credentials.

Run:

```sh
./tests/sudo_lab/run.sh
```

The safe machine-readable evidence is written by default to
`.dev/artifacts/work/sudo-execution/scratch/sudo-lab/evidence.json`. Pass
`--output-dir PATH` to select another artifact directory. Askpass records
contain prompt metadata and synthetic profile names, never credential bytes.
The image declares every non-base dependency in `Dockerfile`; the evidence
records the resolved package and executable versions.
