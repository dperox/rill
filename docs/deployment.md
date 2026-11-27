# Deploy Rill

## Native Windows

The x86-64 Windows GNU release at `7082e92` was cross-compiled on Linux and exercised on Windows 11 Pro. Build it from Linux with the GNU MinGW-w64 x86-64 C compiler and linker installed:

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --release --locked --target x86_64-pc-windows-gnu
```

Copy `target/x86_64-pc-windows-gnu/release/rill.exe` and the sample `public` directory to the Windows host. From their parent directory in PowerShell:

```powershell
$env:DOCUMENT_ROOT = (Resolve-Path .\public).Path
$env:LISTEN = '127.0.0.1:8080'
.\rill.exe config test
if ($LASTEXITCODE -eq 0) { .\rill.exe }
```

In a second shell, run `curl.exe -i http://127.0.0.1:8080/` and verify the expected page. Native checks covered static GET and HEAD, HTTPS, HTTP proxying and policies, TCP and UDP forwarding, and configuration preflight. On Windows, restart Rill to apply a new configuration. Set `RILL_SENDFILE=0` before startup to use buffered file delivery instead of Windows TransmitFile. [Windows file settings](static-io.md#windows-transmitfile)

## Docker

Build from the repository root and publish the container’s default HTTP port.

```sh
docker build -f deploy/Dockerfile -t rill:local .
docker run --rm -p 127.0.0.1:8080:8080 rill:local
```

Open <http://127.0.0.1:8080/>. The image serves `/app/public` as UID/GID
`10001:10001`, listening on `0.0.0.0:8080` inside the container. The image
includes Rill’s MIT license at `/usr/share/doc/rill/LICENSE` and Hyper’s MIT license
at `/usr/share/doc/rill/notices/HYPER-LICENSE`.

### Serve your files

Mount a local `site` directory read-only over the bundled site.

```sh
docker run --rm -p 127.0.0.1:8080:8080 \
  --mount "type=bind,src=$PWD/site,dst=/app/public,readonly" rill:local
```

To package a site in an image, save the following as `Dockerfile.site` next
to `site/` and run `docker build -f Dockerfile.site -t rill:site .`.

```dockerfile
FROM rill:local
COPY site/ /app/site/
ENV DOCUMENT_ROOT=/app/site
```

Make files readable and directories traversable by UID/GID `10001:10001`.
In Kubernetes, use the custom image or mount a site volume read-only at
`/app/public`. Set `DOCUMENT_ROOT` if the volume uses a different mount path.

### Serve multiple sites and inspect requests

For the [two-site example](routing.md#per-host-static-sites), put `rill.json`, `build/app/`, and `build/docs/` under a local `config/` directory. Mount the directory so updated configuration and site files are visible inside the container.

```sh
docker run -d --rm --name rill-sites -p 127.0.0.1:8080:8080 \
  --mount "type=bind,src=$PWD/config,dst=/etc/rill,readonly" \
  -e RILL_CONFIG=/etc/rill/rill.json -e RILL_ACCESS_LOG=1 rill:local
curl -H 'Host: app.example' http://127.0.0.1:8080/
docker logs rill-sites
```

After replacing the configuration, run `docker kill --signal=HUP rill-sites` and check the logs for acceptance or rejection. Stop the example with `docker stop rill-sites`.

### Proxy HTTP

If an HTTP backend named `api` is attached to Docker network `app-net` and
listens on port 9001, forward `/api/` to it.

```sh
docker run --rm --network app-net -p 127.0.0.1:8080:8080 \
  -e UPSTREAMS=api:9001 rill:local
```

Use a reachable backend address. Loopback inside a container refers to that
container. See the [routing guide](routing.md) for host and path rules.

### Serve HTTPS and HTTP/3

Mount a certificate chain and private key read-only. In this example, both
files in `tls/` are readable by UID/GID `10001:10001`.

```sh
docker run --rm \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:8443:8443/tcp -p 127.0.0.1:8443:8443/udp \
  --mount "type=bind,src=$PWD/tls,dst=/run/tls,readonly" \
  -e TLS_LISTEN=0.0.0.0:8443 \
  -e TLS_CERT=/run/tls/tls.crt -e TLS_KEY=/run/tls/tls.key \
  rill:local
```

Use a certificate for the hostname clients request. See the
[TLS and protocol guide](edge.md) for verification.

## Kubernetes

The [sample manifest](../deploy/kubernetes.yaml) runs two replicas and a
ClusterIP Service on TCP port 8080. It serves static files without an
upstream. Readiness and liveness check the HTTP listener over TCP and work
independently of index files, proxy routes and optional listeners.

### Load a local image

For an existing kind cluster, build `rill:local` as above, then run:

```sh
kind load docker-image rill:local
sed 's|image: ghcr.io/dperox/rill:0.1.0|image: rill:local|' deploy/kubernetes.yaml | kubectl apply -f -
kubectl rollout status deployment/rill
kubectl port-forward service/rill 8080:8080
```

Pass `--name CLUSTER` to `kind load docker-image` for a named kind cluster.
For other local clusters, use the cluster’s image-loading command. The
manifest uses `IfNotPresent`. For each rebuild, load a new image tag and
set that tag in the Deployment.

### Pull from a registry

Replace the example registry and tag with a repository the nodes can pull
from.

```sh
docker tag rill:local registry.example.com/team/rill:v1
docker push registry.example.com/team/rill:v1
sed 's|image: ghcr.io/dperox/rill:0.1.0|image: registry.example.com/team/rill:v1|' \
  deploy/kubernetes.yaml | kubectl apply -f -
kubectl rollout status deployment/rill
```

For a private repository, add an `imagePullSecrets` reference in the
Deployment’s pod template for a registry Secret in the same namespace.

### Add a proxy backend

For a backend Service named `api` on port 9001 in the same namespace:

```sh
kubectl set env deployment/rill UPSTREAMS=api:9001
kubectl rollout status deployment/rill
```

Keep the environment settings in your manifest or overlay for subsequent
deployments.

### Add TCP and UDP forwarding

The base manifest enables HTTP only. To restore the optional stream setup
from the earlier `rill-streams` ConfigMap example, keep your populated
ConfigMap and apply the Deployment patch and Service below. For a new setup,
replace the example IP addresses with numeric upstream socket addresses
that the Pods can reach, then create the ConfigMap.

```sh
kubectl create configmap rill-streams \
  --from-literal=RILL_TCP_LISTENERS=0.0.0.0:9000=192.0.2.10:19000 \
  --from-literal=RILL_UDP_LISTENERS=0.0.0.0:5353=192.0.2.20:15353
```

```sh
kubectl patch deployment rill --type=strategic --patch "$(cat <<'STREAMS_PATCH'
spec:
  template:
    spec:
      containers:
        - name: rill
          env:
            - name: RILL_TCP_LISTENERS
              valueFrom:
                configMapKeyRef:
                  name: rill-streams
                  key: RILL_TCP_LISTENERS
            - name: RILL_UDP_LISTENERS
              valueFrom:
                configMapKeyRef:
                  name: rill-streams
                  key: RILL_UDP_LISTENERS
            - name: RILL_STREAM_BALANCE
              valueFrom:
                configMapKeyRef:
                  name: rill-streams
                  key: RILL_STREAM_BALANCE
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
kubectl rollout status deployment/rill
kubectl patch service rill --type=strategic --patch "$(cat <<'STREAMS_SERVICE'
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
read-only and enables TLS on TCP and UDP port 8443. The probes stay on 8080.

```sh
kubectl create secret tls rill-tls --cert=./tls/tls.crt --key=./tls/tls.key
kubectl patch deployment rill --type=strategic --patch "$(cat <<'PATCH'
spec:
  template:
    spec:
      securityContext:
        fsGroup: 10001
      volumes:
        - name: tls
          secret:
            secretName: rill-tls
            defaultMode: 0440
      containers:
        - name: rill
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
kubectl rollout status deployment/rill
kubectl apply -f - <<'SERVICE'
apiVersion: v1
kind: Service
metadata:
  name: rill-tls
spec:
  type: ClusterIP
  selector:
    app: rill
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
HTTP/3 needs a route to the UDP port. After the mounted Secret updates, send SIGHUP to each Rill process to rotate its certificate. A Deployment rollout also picks up the new files. [Graceful reloads](reload.md)
