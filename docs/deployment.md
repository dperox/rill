# Deploy Japka

## Install with Cargo

With stable Rust and Cargo installed, run this from any directory to install [Japka from crates.io](https://crates.io/crates/japka).

```sh
cargo install japka
japka --version
```

Japka serves `./public` in its working directory by default. On Linux or macOS, you can try a temporary site by pointing `DOCUMENT_ROOT` at a directory containing an index page.

```sh
export DOCUMENT_ROOT="$(mktemp -d)"
printf '%s\n' '<h1>Hello from Japka</h1>' > "$DOCUMENT_ROOT/index.html"
export LISTEN=127.0.0.1:8080
japka config test && japka
```

In a second terminal, run `curl --fail http://127.0.0.1:8080/` to retrieve the page. For an existing site, set `DOCUMENT_ROOT` to its directory. For PowerShell commands, see [Native Windows](#native-windows).

## macOS

For a native install, use `cargo install japka`. The [release downloads](https://github.com/dperox/japka/releases/latest) also include archives for Apple silicon and Intel Macs that target macOS 15 and later.

To build your own archive from a source checkout on Apple silicon, run the commands below. Use `amd64` for Intel Macs.

```sh
./deploy/package-macos.sh arm64 dist
(cd dist && shasum -a 256 -c *.sha256 && tar -xzf japka-*-macos-arm64.tar.gz)
cd dist/japka
./japka config test
LISTEN=127.0.0.1:8080 ./japka
```

From another terminal, run `curl --fail http://127.0.0.1:8080/` to retrieve the sample site. To build from a source checkout, run `cargo build --release --locked` and use `target/release/japka`. Native macOS serves static files with buffered reads and supports SIGHUP reloads. [Reload procedure](reload.md)

### Apple containers

On an Apple silicon Mac with Apple’s `container` installed, build and run the Linux ARM64 image in a lightweight VM. [Apple container requirements](https://github.com/apple/container#requirements)

```sh
container system start
container build -f deploy/Dockerfile -t japka:local .
container run --rm japka:local config test
container run --rm -p 127.0.0.1:8080:8080 japka:local
```

Use the same curl request from the Mac. A read-only site mount uses `-v "$PWD/site:/app/public:ro"`. For HTTP/3, configure TLS and publish the TLS port over both TCP and UDP. The macOS 27.2.1 check with Apple container 1.2.0 verified HTTP/1.1, HTTP/2, HTTP/3, and full and range reads from a mounted site. [TLS settings](edge.md) · [Mac benchmarks](../bench/macos.md)

## Native Windows

Install with `cargo install japka`, then open PowerShell in the directory containing your website’s `index.html`.

```powershell
$env:DOCUMENT_ROOT = (Get-Location).Path
$env:LISTEN = '127.0.0.1:8080'
japka config test
if ($LASTEXITCODE -eq 0) { japka }
```

The [release downloads](https://github.com/dperox/japka/releases/latest) also include a Windows x86-64 archive with the executable and sample site. To serve the sample, extract the archive, open a fresh PowerShell window alongside `japka.exe`, and run `.\japka.exe` with the default document root (`.\public`).

In a second shell, run `curl.exe -i http://127.0.0.1:8080/` and verify the expected page. Native checks covered static GET and HEAD, HTTPS, HTTP proxying and policies, TCP and UDP forwarding, and configuration preflight. On Windows, restart Japka to apply a new configuration. Set `JAPKA_SENDFILE=0` before startup to use buffered file delivery instead of Windows TransmitFile. [Windows file settings](static-io.md#windows-transmitfile)

To cross-compile the Windows executable on Linux, install the GNU MinGW-w64 x86-64 C compiler and linker and run the following from the repository root. Cargo writes `japka.exe` to `target/x86_64-pc-windows-gnu/release/`.

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --release --locked --target x86_64-pc-windows-gnu
```

## Docker

Build from the repository root and publish the container’s default HTTP port.

```sh
docker build -f deploy/Dockerfile -t japka:local .
docker run --rm -p 127.0.0.1:8080:8080 japka:local
```

Open <http://127.0.0.1:8080/>. The image serves `/app/public` as UID/GID
`10001:10001`, listening on `0.0.0.0:8080` inside the container. The image
includes Japka’s MIT license at `/usr/share/doc/japka/LICENSE` and Hyper’s MIT license
at `/usr/share/doc/japka/notices/HYPER-LICENSE`.

The release image is available for Linux AMD64 and ARM64 at `ghcr.io/dperox/japka:latest`. Pin its image digest in production for reproducible deployments.

### Serve your files

Mount a local `site` directory read-only over the bundled site.

```sh
docker run --rm -p 127.0.0.1:8080:8080 \
  --mount "type=bind,src=$PWD/site,dst=/app/public,readonly" japka:local
```

To package a site in an image, save the following as `Dockerfile.site` next
to `site/` and run `docker build -f Dockerfile.site -t japka:site .`.

```dockerfile
FROM japka:local
COPY site/ /app/site/
ENV DOCUMENT_ROOT=/app/site
```

Make files readable and directories traversable by UID/GID `10001:10001`.
In Kubernetes, use the custom image or mount a site volume read-only at
`/app/public`. Set `DOCUMENT_ROOT` if the volume uses a different mount path.

### Serve multiple sites and inspect requests

For the [two-site example](routing.md#per-host-static-sites), put `japka.json`, `build/app/`, and `build/docs/` under a local `config/` directory. Mount the directory so updated configuration and site files are visible inside the container.

```sh
docker run -d --rm --name japka-sites -p 127.0.0.1:8080:8080 \
  --mount "type=bind,src=$PWD/config,dst=/etc/japka,readonly" \
  -e JAPKA_CONFIG=/etc/japka/japka.json -e JAPKA_ACCESS_LOG=1 japka:local
curl -H 'Host: app.example' http://127.0.0.1:8080/
docker logs japka-sites
```

After replacing the configuration, run `docker kill --signal=HUP japka-sites` and check the logs for acceptance or rejection. Stop the example with `docker stop japka-sites`.

### Proxy HTTP

If an HTTP backend named `api` is attached to Docker network `app-net` and
listens on port 9001, forward `/api/` to it.

```sh
docker run --rm --network app-net -p 127.0.0.1:8080:8080 \
  -e UPSTREAMS=api:9001 japka:local
```

Use a reachable backend address. Loopback inside a container refers to that
container. See the [routing guide](routing.md) for host and path rules.

### Serve HTTPS and HTTP/3

Mount a certificate chain and private key read-only. In this example, both
files in `tls/` are readable by UID/GID `10001:10001`. Because the cleartext
`LISTEN` port (`0.0.0.0:8080` in containers; `127.0.0.1:8080` or `127.0.0.1:$PORT`
natively) stays active, set `JAPKA_HTTPS_REDIRECT` to redirect HTTP clients to HTTPS.

```sh
docker run --rm \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:8443:8443/tcp -p 127.0.0.1:8443:8443/udp \
  --mount "type=bind,src=$PWD/tls,dst=/run/tls,readonly" \
  -e TLS_LISTEN=0.0.0.0:8443 \
  -e TLS_CERT=/run/tls/tls.crt -e TLS_KEY=/run/tls/tls.key \
  -e JAPKA_HTTPS_REDIRECT=app.example:8443 \
  japka:local
```

Use a certificate for the hostname clients request. See the
[TLS and protocol guide](edge.md) for verification.

## Kubernetes

The [sample manifest](../deploy/kubernetes.yaml) runs two replicas and a
ClusterIP Service on TCP port 8080. It serves static files without an
upstream. Readiness and liveness check the HTTP listener over TCP and work
independently of index files, proxy routes and optional listeners.

### Load a local image

For an existing kind cluster, build `japka:local` as above, then run:

```sh
kind load docker-image japka:local
kubectl apply -f deploy/kubernetes.yaml
kubectl rollout status deployment/japka
kubectl port-forward service/japka 8080:8080
```

Pass `--name CLUSTER` to `kind load docker-image` for a named kind cluster.
For other local clusters, use the cluster’s image-loading command. The
manifest uses `IfNotPresent`. For each rebuild, load a new image tag and
set that tag in the Deployment.

### Pull from a registry

Replace the example registry and tag with a repository the nodes can pull
from.

```sh
docker tag japka:local registry.example.com/team/japka:v1
docker push registry.example.com/team/japka:v1
sed 's|image: japka:local|image: registry.example.com/team/japka:v1|' \
  deploy/kubernetes.yaml | kubectl apply -f -
kubectl rollout status deployment/japka
```

For a private repository, add an `imagePullSecrets` reference in the
Deployment’s pod template for a registry Secret in the same namespace.

### Add a proxy backend

For a backend Service named `api` on port 9001 in the same namespace:

```sh
kubectl set env deployment/japka UPSTREAMS=api:9001
kubectl rollout status deployment/japka
```

Keep the environment settings in your manifest or overlay for subsequent
deployments.

### Add TCP and UDP forwarding

The base manifest enables HTTP only. Create a `japka-streams` ConfigMap with
numeric upstream socket addresses that the Pods can reach, then apply the
Deployment and Service patches below.

```sh
kubectl create configmap japka-streams \
  --from-literal=JAPKA_TCP_LISTENERS=0.0.0.0:9000=192.0.2.10:19000 \
  --from-literal=JAPKA_UDP_LISTENERS=0.0.0.0:5353=192.0.2.20:15353
```

```sh
kubectl patch deployment japka --type=strategic --patch "$(cat <<'STREAMS_PATCH'
spec:
  template:
    spec:
      containers:
        - name: japka
          env:
            - name: JAPKA_TCP_LISTENERS
              valueFrom:
                configMapKeyRef:
                  name: japka-streams
                  key: JAPKA_TCP_LISTENERS
            - name: JAPKA_UDP_LISTENERS
              valueFrom:
                configMapKeyRef:
                  name: japka-streams
                  key: JAPKA_UDP_LISTENERS
            - name: JAPKA_STREAM_BALANCE
              valueFrom:
                configMapKeyRef:
                  name: japka-streams
                  key: JAPKA_STREAM_BALANCE
                  optional: true
          ports:
            - name: tcp-proxy
              containerPort: 9000
              protocol: TCP
            - name: udp-proxy
              containerPort: 5353
              protocol: UDP
STREAMS_PATCH
)"
kubectl rollout status deployment/japka
kubectl patch service japka --type=strategic --patch "$(cat <<'STREAMS_SERVICE'
spec:
  ports:
    - name: tcp-proxy
      port: 9000
      targetPort: tcp-proxy
      protocol: TCP
    - name: udp-proxy
      port: 5353
      targetPort: udp-proxy
      protocol: UDP
STREAMS_SERVICE
)"
```

The Service keeps its existing address and HTTP port. Restart the Deployment
when you change the ConfigMap values.
See the [transport guide](streams.md) for pool and balancing syntax.

### Add HTTPS and HTTP/3

Create a TLS Secret in the Deployment’s namespace. This patch mounts it
read-only, enables TLS on TCP and UDP port 8443, and sets `JAPKA_HTTPS_REDIRECT`
so cleartext HTTP traffic on port 8080 redirects to HTTPS. The TCP probes stay
on 8080 and continue to succeed with redirects enabled.

```sh
kubectl create secret tls japka-tls --cert=./tls/tls.crt --key=./tls/tls.key
kubectl patch deployment japka --type=strategic --patch "$(cat <<'PATCH'
spec:
  template:
    spec:
      securityContext:
        fsGroup: 10001
      volumes:
        - name: tls
          secret:
            secretName: japka-tls
            defaultMode: 0440
      containers:
        - name: japka
          env:
            - name: TLS_LISTEN
              value: "0.0.0.0:8443"
            - name: TLS_CERT
              value: "/run/tls/tls.crt"
            - name: TLS_KEY
              value: "/run/tls/tls.key"
          volumeMounts:
            - name: tls
              mountPath: /run/tls
              readOnly: true
PATCH
)"
kubectl rollout status deployment/japka
kubectl apply -f - <<'SERVICE'
apiVersion: v1
kind: Service
metadata:
  name: japka-tls
spec:
  type: ClusterIP
  selector:
    app: japka
  ports:
    - name: https
      port: 8443
      targetPort: 8443
      protocol: TCP
    - name: http3
      port: 8443
      targetPort: 8443
      protocol: UDP
SERVICE
```

Use the TLS Service from inside the cluster. For external HTTP/3 clients,
use a load balancer or gateway that forwards both transports. Preserve
the advertised QUIC port. `kubectl port-forward` can test HTTPS over TCP.
HTTP/3 needs a route to the UDP port. After the mounted Secret updates, send SIGHUP to each Japka process to rotate its certificate. A Deployment rollout also picks up the new files. [Graceful reloads](reload.md)
