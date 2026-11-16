# Native Cloud capture

This standalone crate owns bounded Claude native capture, private disk spooling,
a durable many-session queue, and upload reconciliation through a caller-owned
transport. It contains no network client.

The default build has no capture API. Select the explicit non-default `cloud`
feature for controlled integration. Production upload enablement, consent,
credentials, service endpoints, and scheduling belong to the native host.

Read the [v1 contract](../../docs/cloud/capture-v1.md) for source ownership,
fixed 8 MiB expanded chunks, metadata, limits, acceptance, and platform scope.
Tests use only the [public synthetic fixture package](../../fixtures/cloud/capture-v1)
and files generated inside temporary directories.
