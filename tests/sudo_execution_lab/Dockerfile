# `source` compiles the vendored checkout in this build stage; `prebuilt`
# takes the binaries that `run.py --cached-build` built with the same pinned
# Rust image and placed under bin/ in the context.
ARG BINARIES=source

FROM rust:1.98-slim-trixie@sha256:f47a8de237dcbb0b0ce1099901e60a89728e3d51f24e664b40e947171538ade7 AS source
WORKDIR /build/agentenv
# Stripped, debuginfo-free binaries keep the helper within the 64 MiB
# deployment upload bound; the lab exercises behaviour, not symbols.
ENV CARGO_HTTP_TIMEOUT=30 CARGO_NET_RETRY=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_STRIP=true
COPY Cargo.toml Cargo.lock build.rs ./
COPY vendor/ /build/vendor/
COPY src/ ./src/
COPY tests/fixtures/bin/ ./tests/fixtures/bin/
RUN --mount=type=cache,target=/build/agentenv/target \
    cargo --offline --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="/build/vendor"' \
    build --locked --bins --features test-support \
    && mkdir /out \
    && cp /build/agentenv/target/debug/agentenv /build/agentenv/target/debug/agentenv-sudo-helper /build/agentenv/target/debug/agentenv-ssh-askpass /out/

FROM scratch AS prebuilt
COPY bin/ /out/

FROM ${BINARIES} AS binaries

FROM agentenv-sudo-lab:s0
COPY --from=binaries /out/agentenv /opt/agentenv/agentenv
COPY --from=binaries /out/agentenv-sudo-helper /opt/agentenv/agentenv-sudo-helper
COPY --from=binaries /out/agentenv-ssh-askpass /opt/agentenv/agentenv-ssh-askpass
COPY tests/sudo_execution_lab/verify.py /opt/sudo-lab/verify_agentenv.py
COPY tests/sudo_execution_lab/verify_ssh.py /opt/sudo-lab/verify_ssh.py
COPY tests/sudo_execution_lab/verify_release.py /opt/sudo-lab/verify_release.py
COPY --chmod=0755 tests/sudo_execution_lab/target.py /opt/sudo-lab/target.py
ENTRYPOINT ["python3", "/opt/sudo-lab/verify_agentenv.py"]
