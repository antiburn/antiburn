# Native Cloud capture

This standalone crate owns bounded Claude native capture, private disk spooling,
a durable many-session queue, and upload reconciliation through a caller-owned
transport. It contains no network client.

The default build has no capture API. Select the explicit non-default `cloud`
feature for controlled integration. Production upload enablement, consent,
credentials, service endpoints, and scheduling belong to the native host.

Read the [v1 contract](../../docs/cloud/capture-v1.md) for source ownership,
independent stream chunks with fixed 8 MiB expanded bounds, stable source
fingerprints, typed metadata sections, frozen upload plans, limits, acceptance,
and platform scope. Use `resume_step` to bound each scheduler turn to one saved
chunk. The host owns persistent selections, accepted fingerprints, and fair
scheduling.
After acceptance, save the accepted fingerprint and terminal request durably,
then call `acknowledge_accepted` to remove the retained receipt.
Tests use only the [public synthetic fixture package](../../fixtures/cloud/capture-v1)
and files generated inside temporary directories.
