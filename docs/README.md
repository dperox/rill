# Japka documentation

Japka serves static websites, forwards HTTP requests to your applications, and balances traffic across backends.

## Get running

Let your AI help you set up Japka. Copy this prompt into your AI assistant.

```text
Help me set up Japka. Ask where I want to run it (Cargo, Docker, or Kubernetes),
what files or application I want to serve, and what URL should reach it.
Use the official guide at https://github.com/dperox/japka/blob/main/docs/deployment.md.

Prepare the simplest setup that fits my answers. Use the target Japka binary’s
help and config schema to check which settings it accepts. If I already run
Japka, inspect the effective configuration, preserve my existing routes, and
use japka config plan to show what your proposed file would change.

Validate the proposal with japka config test using the intended environment
and mounts. Show me the configuration, the commands to deploy it, and a curl
request with the response I should expect. Tell me which checks you ran.
Wait for my approval before applying the configuration or starting the service.
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
