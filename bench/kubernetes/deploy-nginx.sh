#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 1 || -z ${KUBECONFIG:-} ]] || [[ $(kubectl config current-context) != kind-rill-benchmark ]]; then
  echo "Usage: KUBECONFIG=/path/to/lab/kubeconfig $0 /path/to/lab-state" >&2
  exit 2
fi

here=$(cd -- "$(dirname -- "$0")" && pwd)
state=$1
umask 077
printf 'rill-bench localhost\nnginx-bench localhost\n' >"$state/hosts"
if [[ ! -s "$state/nginx.pem" || ! -s "$state/nginx.key" ]]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj /CN=nginx-bench \
    -addext subjectAltName=DNS:nginx-bench -keyout "$state/nginx.key" -out "$state/nginx.pem"
fi
kubectl -n rill-bench create secret tls nginx-bench-tls \
  --cert "$state/nginx.pem" --key "$state/nginx.key" --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f "$here/nginx.yaml"
kubectl -n rill-bench rollout status deploy/nginx --timeout=180s
