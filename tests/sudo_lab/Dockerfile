FROM postgres:17@sha256:67f41722b7a8cbdb868a44a4995c846eddfdc2973bccb291ce937dce88ad5675

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        openssh-client \
        openssh-server \
        procps \
        python3 \
        strace \
        sudo \
    && rm -rf /var/lib/apt/lists/*

COPY askpass.py /opt/sudo-lab/askpass.py
COPY run_lab.py /opt/sudo-lab/run_lab.py
COPY bin/ /opt/sudo-lab/bin/

RUN chmod 0755 /opt/sudo-lab/askpass.py /opt/sudo-lab/run_lab.py /opt/sudo-lab/bin/*

ENTRYPOINT ["/opt/sudo-lab/run_lab.py"]
