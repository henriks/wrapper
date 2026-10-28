FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash ca-certificates curl git iptables \
    openssh-client procps python3 unzip util-linux xz-utils \
    && rm -rf /var/lib/apt/lists/*
# Install the current stable engine and CLI plugins from Docker's signed repo.
RUN install -m 0755 -d /etc/apt/keyrings \
    && curl --fail --show-error --location https://download.docker.com/linux/debian/gpg -o /etc/apt/keyrings/docker.asc \
    && chmod a+r /etc/apt/keyrings/docker.asc \
    && printf 'deb [arch=%s signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/debian bookworm stable\n' "$(dpkg --print-architecture)" > /etc/apt/sources.list.d/docker.list \
    && apt-get update && apt-get install -y --no-install-recommends \
       docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin \
    && rm -rf /var/lib/apt/lists/*
# Mise manages project toolchains and Codex; keep its binary outside HOME.
RUN curl --fail --show-error --location https://mise.run -o /tmp/install-mise \
    && MISE_INSTALL_PATH=/usr/local/bin/mise sh /tmp/install-mise \
    && rm /tmp/install-mise
RUN mkdir -p /home/agent /var/lib/docker
COPY guest/daemon guest/run guest/hostctl /usr/local/lib/sandbox-wrap/
COPY host_rpc.py network_proxy.py /usr/local/lib/sandbox-wrap/
RUN chmod +x /usr/local/lib/sandbox-wrap/*
RUN ln -s /usr/local/lib/sandbox-wrap/hostctl /usr/local/bin/hostctl
ENV HOME=/home/agent DOCKER_HOST=unix:///run/docker.sock
CMD ["/usr/local/lib/sandbox-wrap/daemon"]
