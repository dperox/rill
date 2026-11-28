# Japka documentation

Japka serves static websites, forwards HTTP requests to your applications, and balances traffic across backends.

## Get running

Let your AI help you set up and configure Japka. Copy this prompt into your AI assistant.

```text
Help me configure Japka for a new or existing deployment. Ask where it will
run, which hostnames and paths should serve files or forward requests, and
which directories or backends to use.

Use the configuration guide to prepare a change I can review.
https://github.com/dperox/japka/blob/main/docs/config-review.md
For installation with Cargo, Docker, or Kubernetes, use the deployment guide.
https://github.com/dperox/japka/blob/main/docs/deployment.md

Use japka config list --json for startup settings and japka config schema
for the JSON file. For an existing service, run japka config show --json in
its environment and preserve unrelated settings and inherited routes.

Draft a separate candidate file. Point JAPKA_CONFIG at it for japka config test
using the service’s working directory, environment, and mounts. If there is an
active file, run japka config plan BASE CANDIDATE --json with both file paths.
Show me what changes, which settings need a reload or restart, and a curl
request with the expected response. Wait for my approval before applying the
configuration or starting the service.
```

| Where are you running Japka? | Start here |
| --- | --- |
| On your machine | [Install with Cargo and serve your first page](deployment.md#install-with-cargo) |
| In a container | [Run the published image or mount your own site](deployment.md#docker) |
| In a Kubernetes cluster | [Deploy two replicas and verify the Service](deployment.md#kubernetes) |

## Configure and operate

- [Route requests and balance traffic](routing.md) by hostname or path, add backends, and configure health checks.
- [Review a configuration change](config-review.md) to see what an agent’s proposal would change before approving deployment. [Reload an approved file on Unix](reload.md).
- [Enable HTTPS, HTTP/2, and HTTP/3](edge.md) and configure per-host certificates.
- [Migrate an NGINX configuration](migration.md) and review the differences before switching traffic.

For specialized deployments, see [access policies](policy.md), [static file delivery](static-io.md), and [TCP, UDP, and mail forwarding](streams.md).
