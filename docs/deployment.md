# Deploy Japka

Install the CLI with Cargo to serve files from your machine, run the published container for a bundled sample site, or deploy the release manifest to Kubernetes. Each example ends with an HTTP request so you can verify what a client receives.

## Install with Cargo

With a stable Rust toolchain on your path, install Japka from crates.io. You can run this from any directory.

```sh
cargo install japka
japka --version
```

To serve an existing site, open a terminal in the directory containing your **public website files**. Check that Japka can read the site, then start the server.

```sh
DOCUMENT_ROOT=. japka config test && DOCUMENT_ROOT=. japka
```

If you don’t have a site yet, create a temporary directory with a single page and serve it.

```sh
export DOCUMENT_ROOT="$(mktemp -d)"
printf '%s\n' 'Hello from Japka' > "$DOCUMENT_ROOT/index.html"
japka config test && japka
```

With the server running, use a second terminal to fetch the page. The sample returns `Hello from Japka`. Press Ctrl-C in the server terminal to stop it.

```sh
curl --fail http://127.0.0.1:8080/
```

Japka serves `./public` by default. Set `DOCUMENT_ROOT` to change the site directory and `LISTEN` to change the bind address. To send requests under `/api/` to an application on the same machine, start Japka with `UPSTREAMS=127.0.0.1:9001` and your usual document root. [Routing and load balancing](routing.md)

## Docker

The published image includes a sample site and supports Linux AMD64 and ARM64. This command exposes it on port 8080 on your machine.

```sh
docker run --rm --name japka -p 127.0.0.1:8080:8080 ghcr.io/dperox/japka:latest
```

In a second terminal, run `curl --fail http://127.0.0.1:8080/`. Look for **Japka is running** in the response. Run `docker stop japka` before starting another example on the same port.

### Serve your own site

From the directory containing your public website files, mount that directory read-only over the image’s document root. The image runs as UID/GID `10001:10001` and needs permission to read files and traverse directories.

```sh
docker run --rm -p 127.0.0.1:8080:8080 \
  --mount "type=bind,src=$PWD,dst=/app/public,readonly" \
  ghcr.io/dperox/japka:latest
```

For a reverse proxy, attach Japka to the backend’s Docker network. For example, add `--network app-net -e UPSTREAMS=api:9001` before the image name if the backend is reachable as `api` on that network.

### Use a configuration file

If your routes are in `config/japka.json`, mount the directory and validate the file inside the image. Include any site or certificate mounts the server will use.

```sh
docker run --rm --mount "type=bind,src=$PWD/config,dst=/etc/japka,readonly" \
  -e JAPKA_CONFIG=/etc/japka/japka.json ghcr.io/dperox/japka:latest config test
```

To start the server, keep the mounts and environment, add `-p 127.0.0.1:8080:8080` before the image name, and omit the trailing `config test`.

## Kubernetes

The release provides a Deployment with two replicas and a ClusterIP Service on TCP port 8080. You need `kubectl` and a cluster whose nodes can pull the GHCR image. Download the manifest to your deployment directory and check which cluster you are targeting.

```sh
curl -fL -o kubernetes.yaml https://github.com/dperox/japka/releases/latest/download/kubernetes.yaml
kubectl config current-context
```

Review the manifest and choose an existing namespace. The example uses `default`. For a reproducible production rollout, replace the image’s `:latest` tag in the manifest with the release image digest before applying it.

```sh
namespace=default
kubectl -n "$namespace" apply -f kubernetes.yaml
kubectl -n "$namespace" rollout status deployment/japka
kubectl -n "$namespace" get deployment japka
kubectl -n "$namespace" port-forward service/japka 8080:8080
```

Check that the Deployment reports **2/2** ready. Leave the port-forward running and use `curl --fail http://127.0.0.1:8080/` in another terminal to verify the bundled page. The Service is reachable inside the cluster as `japka:8080` from the same namespace.

To serve your own site, mount a read-only volume at `/app/public` or supply an image containing your files. To proxy to a Service named `api` in the same namespace, add `UPSTREAMS` with value `api:9001` to the Japka container’s `env` in the manifest. Apply the edited file and check an application URL under `/api/`.

## Review configuration changes

Run `japka config test` with the same working directory, environment, and files that the service will use. To review a proposed JSON file, run `japka config plan active.json candidate.json --json` and inspect its `review` entries. Unix processes can reload supported file settings with SIGHUP. Changes to startup environment variables require a new process. [Configuration review walkthrough](config-review.md) · [Reload procedure](reload.md)

## Other deployment options

- **Prebuilt binaries.** The release archives include the executable and sample site. Extract the archive and start Japka from the directory containing `public/`. [Release downloads](https://github.com/dperox/japka/releases/latest)
- **Source builds and local clusters.** From the repo root, use `cargo build --release --locked` for the native binary or `docker build -f deploy/Dockerfile -t japka:local .` for an image. For kind, load that image with `kind load docker-image japka:local` and apply the [checkout manifest](../deploy/kubernetes.yaml), which selects the local image.
- **TLS and additional ports.** HTTPS uses TCP. HTTP/3 also needs UDP on the TLS port. For TCP or UDP forwarding, expose each listener’s port and protocol on the Service. [HTTPS and HTTP/3 on Kubernetes](edge.md#kubernetes-tls-and-http3) · [TCP and UDP forwarding](streams.md#kubernetes)

### Apple containers

On Apple silicon, Apple’s `container` can run the same GHCR image. Start the runtime with `container system start`, then use `container run --rm -p 127.0.0.1:8080:8080 ghcr.io/dperox/japka:latest`.

### Native Windows

In PowerShell, install with `cargo install japka`. Open the public site directory, validate the configuration, and start the server.

```powershell
$env:DOCUMENT_ROOT = (Get-Location).Path
japka config test
if ($LASTEXITCODE -eq 0) { japka }
```

Verify the page with `curl.exe --fail http://127.0.0.1:8080/` from another terminal. Restart the process after changing its configuration.
