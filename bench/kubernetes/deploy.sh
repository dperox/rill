#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 1 || -z ${KUBECONFIG:-} ]]; then
  echo "Usage: KUBECONFIG=/path/to/isolated/kubeconfig $0 /path/to/lab-state" >&2
  exit 2
fi
if [[ $(kubectl config current-context) != kind-rill-benchmark ]]; then
  echo "Expected the kind-rill-benchmark context" >&2
  exit 2
fi

here=$(cd -- "$(dirname -- "$0")" && pwd)
state=$1
umask 077
mkdir -p "$state"
kubectl create namespace rill-bench --dry-run=client -o yaml | kubectl apply -f -
kubectl create namespace traefik --dry-run=client -o yaml | kubectl apply -f -

${HELM:-helm} template traefik traefik --repo https://traefik.github.io/charts \
  --version 43.0.0 --namespace traefik -f "$here/traefik.yaml" | kubectl apply -f -

if [[ ! -s "$state/tls.pem" || ! -s "$state/tls.key" ]]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj /CN=rill-bench \
    -addext subjectAltName=DNS:rill-bench -keyout "$state/tls.key" -out "$state/tls.pem"
fi
printf 'rill-bench localhost\n' >"$state/hosts"
python3 - "$state" <<'PY'
import sys
from pathlib import Path

directory = Path(sys.argv[1])
for name, byte, size in (("static", b"S", 4096), ("api", b"A", 4096), ("download", b"D", 262144)):
    (directory / f"{name}.bin").write_bytes(byte * size)
PY

kubectl -n rill-bench create secret tls rill-bench-tls \
  --cert "$state/tls.pem" --key "$state/tls.key" --dry-run=client -o yaml | kubectl apply -f -
kubectl -n rill-bench create configmap fixtures \
  --from-file="$state/static.bin" --from-file="$state/api.bin" --from-file="$state/download.bin" \
  --dry-run=client -o yaml | kubectl apply --server-side -f -
kubectl apply -f "$here/app.yaml"
kubectl -n traefik rollout status deploy/traefik --timeout=180s
for name in api-origin download-origin rill; do
  kubectl -n rill-bench rollout status "deploy/$name" --timeout=180s
done
