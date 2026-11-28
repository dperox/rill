# Japka documentation

Japka serves static websites, forwards HTTP requests to your applications, and balances traffic across backends.

## Get running

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
