# Docker inside the development VM

The Dockerfile installs Engine, CLI, Buildx and the `docker compose` plugin from
[Docker's official stable apt repository](https://docs.docker.com/engine/install/debian/).
Smolvm starts
`guest/daemon` as the image workload on every boot. It runs `dockerd` on the guest's
Unix socket; the host Docker socket is never passed into the VM.

The wrapper provisions `--volume /var/lib/docker`, a guest-local persistent volume
on smolvm's storage disk. This avoids putting Docker's image/container data on either
the root overlay or the host virtiofs workspace. Images, containers, and named
volumes survive VM restarts. `--reset` removes them with the rest of the VM.

`guest/prepare` is sent over exec stdin on every boot and waits for Docker
readiness before the foreground command starts. It applies user/socket setup;
host exports enforce filesystem policy before the VM starts.
The command runs with the invoking user's UID/GID and membership in the guest
Docker group. Containers can bind-mount the project at its original host path,
which is also present at that path inside the guest.

Raw networking uses smolvm's virtio-net backend; default egress goes through the
host CONNECT proxy. Docker bridge networking works within
the VM. Publish a service from Docker to a guest port, then pass the corresponding
`--port HOST:GUEST` mapping to the wrapper to reach it from the host. For example,
launch `./codex-wrap --port 8080:8080`, then run `docker run -p 8080:80 nginx` inside.

After the last session exits or terminates, the supervisor stops the VM gracefully. Docker's guest
workload receives its stop signal and waits for the daemon to terminate, allowing
its storage to flush. The guest's `/tmp` and `/run` are ephemeral.

Docker access grants guest root privileges. All shared host directories are
prepared in a host mount namespace, enforcing Git/extra-share read-only policy
and excluding `.sandbox` independently of guest mount flags. Privileged Docker
containers cannot recover the hidden state with non-recursive bind mounts or
change host read-only mounts by remounting them in the guest. Writable shared
files remain intentionally writable. See [README.md](README.md) for host namespace
setup, lifecycle behavior and the Ubuntu helper/profile.
